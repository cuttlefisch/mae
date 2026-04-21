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
        normal.bind(parse_key_seq("^"), "move-to-first-non-blank");
        normal.bind(parse_key_seq("_"), "move-to-first-non-blank");
        normal.bind(parse_key_seq("+"), "move-line-next-non-blank");
        normal.bind(parse_key_seq("-"), "move-line-prev-non-blank");
        normal.bind(parse_key_seq("G"), "move-to-last-line");
        normal.bind(parse_key_seq("gg"), "move-to-first-line");
        // Display-line motions (word wrap aware)
        normal.bind(parse_key_seq("gj"), "move-display-down");
        normal.bind(parse_key_seq("gk"), "move-display-up");
        normal.bind(parse_key_seq("g0"), "move-display-line-start");
        normal.bind(parse_key_seq("g$"), "move-display-line-end");
        // Word motions
        normal.bind(parse_key_seq("w"), "move-word-forward");
        normal.bind(parse_key_seq("b"), "move-word-backward");
        normal.bind(parse_key_seq("e"), "move-word-end");
        normal.bind(parse_key_seq("W"), "move-big-word-forward");
        normal.bind(parse_key_seq("B"), "move-big-word-backward");
        normal.bind(parse_key_seq("E"), "move-big-word-end");
        normal.bind(parse_key_seq("ge"), "move-word-end-backward");
        normal.bind(parse_key_seq("gE"), "move-big-word-end-backward");
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
        // Aliases (D/Y/X)
        normal.bind(parse_key_seq("D"), "delete-to-line-end");
        normal.bind(parse_key_seq("Y"), "yank-line");
        normal.bind(parse_key_seq("X"), "delete-char-backward");
        // Repeat find (;/,)
        normal.bind(parse_key_seq(";"), "repeat-find");
        normal.bind(parse_key_seq(","), "repeat-find-reverse");
        // Reselect visual (gv)
        normal.bind(parse_key_seq("gv"), "reselect-visual");
        // Search
        normal.bind(parse_key_seq("/"), "search-forward-start");
        normal.bind(parse_key_seq("?"), "search-backward-start");
        normal.bind(parse_key_seq("n"), "search-next");
        normal.bind(parse_key_seq("N"), "search-prev");
        normal.bind(parse_key_seq("*"), "search-word-under-cursor");
        normal.bind(parse_key_seq("#"), "search-word-under-cursor-backward");
        // gn / gN — select next/prev search match as visual selection.
        // Operator variants: dgn, cgn, ygn (and capital-N backward equivalents).
        // Practical Vim tip 86: `cgn<text><Esc>` + `.` for single-key global replace.
        normal.bind(parse_key_seq("gn"), "visual-select-next-match");
        normal.bind(parse_key_seq("gN"), "visual-select-prev-match");
        normal.bind(parse_key_seq("dgn"), "delete-next-match");
        normal.bind(parse_key_seq("dgN"), "delete-prev-match");
        normal.bind(parse_key_seq("cgn"), "change-next-match");
        normal.bind(parse_key_seq("cgN"), "change-prev-match");
        normal.bind(parse_key_seq("ygn"), "yank-next-match");
        normal.bind(parse_key_seq("ygN"), "yank-prev-match");
        // Editing
        normal.bind(parse_key_seq("x"), "delete-char-forward");
        normal.bind(parse_key_seq("dd"), "delete-line");
        // Operator-pending: bare d/c/y enter pending state, compose with any motion
        normal.bind(parse_key_seq("d"), "operator-delete");
        normal.bind(parse_key_seq("c"), "operator-change");
        normal.bind(parse_key_seq("y"), "operator-yank");
        // Text object operators (kept — text objects use their own flow)
        normal.bind(parse_key_seq("di"), "delete-inner-object");
        normal.bind(parse_key_seq("da"), "delete-around-object");
        normal.bind(parse_key_seq("ci"), "change-inner-object");
        normal.bind(parse_key_seq("ca"), "change-around-object");
        normal.bind(parse_key_seq("yi"), "yank-inner-object");
        normal.bind(parse_key_seq("ya"), "yank-around-object");
        // Linewise specials (kept — these operate on whole lines)
        normal.bind(parse_key_seq("cc"), "change-line");
        normal.bind(parse_key_seq("C"), "change-to-line-end");
        // Replace
        normal.bind(parse_key_seq("r"), "replace-char-await");
        // Substitute (Practical Vim tip 2 — single-key `xi` / `cc` shortcuts)
        normal.bind(parse_key_seq("s"), "substitute-char");
        normal.bind(parse_key_seq("S"), "substitute-line");
        // Re-enter insert at last insert-exit position
        normal.bind(parse_key_seq("gi"), "reinsert-at-last-position");
        // Jump list (Practical Vim ch. 9).
        // NOTE: when the focused buffer is a Help buffer, key_handling.rs
        // intercepts C-o / C-i before keymap lookup and routes them to
        // help-back / help-forward. Everywhere else these drive the
        // vim-style jump list.
        normal.bind(parse_key_seq("C-o"), "jump-backward");
        normal.bind(parse_key_seq("C-i"), "jump-forward");
        // Change list (Practical Vim ch. 9): g; walks back through edit
        // positions, g, walks forward. Symmetric to Ctrl-o / Ctrl-i but
        // scoped to edit locations, not motion targets.
        normal.bind(parse_key_seq("g;"), "change-backward");
        normal.bind(parse_key_seq("g,"), "change-forward");
        // gf — open file under cursor. Resolves absolute paths, relative
        // paths (cwd first, then buffer's dir), and ~-expanded home paths.
        normal.bind(parse_key_seq("gf"), "goto-file-under-cursor");
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
        // Font zoom (GUI)
        normal.bind(vec![KeyPress::ctrl('=')], "increase-font-size");
        normal.bind(vec![KeyPress::ctrl('-')], "decrease-font-size");
        normal.bind(vec![KeyPress::ctrl('0')], "reset-font-size");
        // Alternate file
        normal.bind(vec![KeyPress::ctrl('6')], "alternate-file");
        // Dot repeat
        normal.bind(parse_key_seq("."), "dot-repeat");
        // ZZ/ZQ (vim save-and-quit / force-quit)
        normal.bind(parse_key_seq("ZZ"), "save-and-quit");
        normal.bind(parse_key_seq("ZQ"), "force-quit");
        // Yank/Paste
        normal.bind(parse_key_seq("yy"), "yank-line");
        normal.bind(parse_key_seq("p"), "paste-after");
        normal.bind(parse_key_seq("P"), "paste-before");
        // Register prompt: `"<char>` selects the register for the next
        // yank/delete/paste. Resolved by the key-handling layer via
        // `pending_register_prompt` — dispatch just arms the flag.
        normal.bind(parse_key_seq("\""), "prompt-register");
        // Surrounds (vim-surround)
        normal.bind(parse_key_seq("ds"), "delete-surround-await");
        normal.bind(parse_key_seq("cs"), "change-surround-await");
        normal.bind(parse_key_seq("ys"), "operator-surround");
        normal.bind(parse_key_seq("yss"), "surround-line-await");
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
        normal.bind(parse_key_seq_spaced("SPC :"), "enter-command-mode");
        // +buffer (Doom parity)
        normal.bind(parse_key_seq_spaced("SPC b s"), "save");
        normal.bind(parse_key_seq_spaced("SPC b b"), "switch-buffer");
        normal.bind(parse_key_seq_spaced("SPC b d"), "kill-buffer");
        normal.bind(parse_key_seq_spaced("SPC b n"), "next-buffer");
        normal.bind(parse_key_seq_spaced("SPC b p"), "prev-buffer");
        normal.bind(parse_key_seq_spaced("SPC b l"), "alternate-file");
        normal.bind(parse_key_seq_spaced("SPC b m"), "view-messages");
        normal.bind(parse_key_seq_spaced("SPC b N"), "new-buffer");
        normal.bind(parse_key_seq_spaced("SPC b D"), "force-kill-buffer");
        // +file
        normal.bind(parse_key_seq_spaced("SPC f f"), "find-file");
        // Ranger/dired-style directory browser: spatial traversal
        // complement to the fuzzy `SPC f f` picker. (`-` would be the
        // vim/dirvish convention, but it's already bound to
        // `move-line-prev-non-blank` — keep the motion primitive.)
        normal.bind(parse_key_seq_spaced("SPC f d"), "file-browser");
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
        normal.bind(parse_key_seq_spaced("SPC a a"), "open-ai-agent");
        normal.bind(parse_key_seq_spaced("SPC a p"), "ai-prompt");
        normal.bind(parse_key_seq_spaced("SPC a c"), "ai-cancel");
        normal.bind(parse_key_seq_spaced("SPC a m"), "ai-set-mode");
        normal.bind(parse_key_seq_spaced("SPC a P"), "ai-set-profile");
        // +help
        normal.bind(parse_key_seq_spaced("SPC h h"), "help");
        normal.bind(parse_key_seq_spaced("SPC h k"), "describe-key");
        normal.bind(parse_key_seq_spaced("SPC h c"), "describe-command");
        normal.bind(parse_key_seq_spaced("SPC h o"), "describe-option");
        normal.bind(parse_key_seq_spaced("SPC h t"), "tutor");
        normal.bind(parse_key_seq_spaced("SPC h s"), "help-search");
        normal.bind(parse_key_seq_spaced("SPC h b"), "help-back");
        normal.bind(parse_key_seq_spaced("SPC h f"), "help-forward");
        normal.bind(parse_key_seq_spaced("SPC h q"), "help-close");
        normal.bind(parse_key_seq_spaced("SPC h l"), "help-reopen");
        normal.bind(parse_key_seq_spaced("SPC h d"), "dashboard");
        // +scratch
        normal.bind(parse_key_seq_spaced("SPC x"), "toggle-scratch-buffer");
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
        normal.bind(parse_key_seq_spaced("SPC d p"), "debug-panel");
        // +quit
        normal.bind(parse_key_seq_spaced("SPC q q"), "quit");
        normal.bind(parse_key_seq_spaced("SPC q Q"), "force-quit");
        // +search/syntax (tree-sitter structural selection + search)
        normal.bind(parse_key_seq_spaced("SPC s s"), "search-buffer");
        normal.bind(parse_key_seq_spaced("SPC s n"), "syntax-select-node");
        normal.bind(parse_key_seq_spaced("SPC s e"), "syntax-expand-selection");
        normal.bind(parse_key_seq_spaced("SPC s c"), "syntax-contract-selection");
        normal.bind(parse_key_seq_spaced("SPC s p"), "project-search");
        normal.bind(parse_key_seq_spaced("SPC s h"), "clear-search-highlight");
        // SPC / — Doom shortcut for project search
        normal.bind(parse_key_seq_spaced("SPC /"), "project-search");
        // +eval (Scheme REPL / lisp machine)
        normal.bind(parse_key_seq_spaced("SPC e l"), "eval-line");
        normal.bind(parse_key_seq_spaced("SPC e b"), "eval-buffer");
        normal.bind(parse_key_seq_spaced("SPC e o"), "open-scheme-repl");
        normal.bind(parse_key_seq_spaced("SPC e s"), "send-to-shell");

        // +project
        normal.bind(parse_key_seq_spaced("SPC p f"), "project-find-file");
        normal.bind(parse_key_seq_spaced("SPC p s"), "project-search");
        normal.bind(parse_key_seq_spaced("SPC p d"), "project-browse");
        normal.bind(parse_key_seq_spaced("SPC p r"), "project-recent-files");
        normal.bind(parse_key_seq_spaced("SPC p p"), "project-switch");
        // +file expansions
        normal.bind(parse_key_seq_spaced("SPC f r"), "recent-files");
        normal.bind(parse_key_seq_spaced("SPC f y"), "yank-file-path");
        normal.bind(parse_key_seq_spaced("SPC f R"), "rename-file");
        normal.bind(parse_key_seq_spaced("SPC f c"), "edit-config");
        normal.bind(parse_key_seq_spaced("SPC f C"), "edit-settings");
        normal.bind(parse_key_seq_spaced("SPC f S"), "save-as");
        // +buffer expansions
        normal.bind(parse_key_seq_spaced("SPC b o"), "kill-other-buffers");
        normal.bind(parse_key_seq_spaced("SPC b S"), "save-all-buffers");
        normal.bind(parse_key_seq_spaced("SPC b r"), "revert-buffer");
        // +toggle expansions
        normal.bind(parse_key_seq_spaced("SPC t l"), "toggle-line-numbers");
        normal.bind(
            parse_key_seq_spaced("SPC t r"),
            "toggle-relative-line-numbers",
        );
        normal.bind(parse_key_seq_spaced("SPC t w"), "toggle-word-wrap");
        normal.bind(parse_key_seq_spaced("SPC t F"), "toggle-fps");
        normal.bind(parse_key_seq_spaced("SPC t D"), "debug-mode");
        // +git
        normal.bind(parse_key_seq_spaced("SPC g s"), "git-status");
        normal.bind(parse_key_seq_spaced("SPC g b"), "git-blame");
        normal.bind(parse_key_seq_spaced("SPC g d"), "git-diff");
        normal.bind(parse_key_seq_spaced("SPC g l"), "git-log");
        // +open
        normal.bind(parse_key_seq_spaced("SPC o t"), "terminal");
        normal.bind(parse_key_seq_spaced("SPC o r"), "terminal-reset");
        normal.bind(parse_key_seq_spaced("SPC o c"), "terminal-close");
        // +notes (KB shortcuts)
        normal.bind(parse_key_seq_spaced("SPC n f"), "kb-find");
        // +code (LSP shortcuts)
        normal.bind(parse_key_seq_spaced("SPC c d"), "lsp-goto-definition");
        normal.bind(parse_key_seq_spaced("SPC c r"), "lsp-find-references");
        normal.bind(parse_key_seq_spaced("SPC c k"), "lsp-hover");
        normal.bind(parse_key_seq_spaced("SPC c x"), "lsp-show-diagnostics");
        normal.bind(parse_key_seq_spaced("SPC c a"), "lsp-code-action");
        normal.bind(parse_key_seq_spaced("SPC c R"), "lsp-rename");
        normal.bind(parse_key_seq_spaced("SPC c f"), "lsp-format");

        // Group labels for which-key popup
        normal.set_group_name(parse_key_seq_spaced("SPC b"), "+buffer");
        normal.set_group_name(parse_key_seq_spaced("SPC f"), "+file");
        normal.set_group_name(parse_key_seq_spaced("SPC w"), "+window");
        normal.set_group_name(parse_key_seq_spaced("SPC a"), "+ai");
        normal.set_group_name(parse_key_seq_spaced("SPC t"), "+toggle");
        normal.set_group_name(parse_key_seq_spaced("SPC d"), "+debug");
        normal.set_group_name(parse_key_seq_spaced("SPC h"), "+help");
        normal.set_group_name(parse_key_seq_spaced("SPC q"), "+quit");
        normal.set_group_name(parse_key_seq_spaced("SPC s"), "+search/syntax");
        normal.set_group_name(parse_key_seq_spaced("SPC e"), "+eval");
        normal.set_group_name(parse_key_seq_spaced("SPC c"), "+code");
        normal.set_group_name(parse_key_seq_spaced("SPC p"), "+project");
        normal.set_group_name(parse_key_seq_spaced("SPC g"), "+git");
        normal.set_group_name(parse_key_seq_spaced("SPC n"), "+notes");
        normal.set_group_name(parse_key_seq_spaced("SPC o"), "+open");

        let mut insert = Keymap::new("insert");
        insert.bind(vec![KeyPress::special(Key::Escape)], "enter-normal-mode");
        insert.bind(vec![KeyPress::special(Key::Left)], "move-left");
        insert.bind(vec![KeyPress::special(Key::Down)], "move-down");
        insert.bind(vec![KeyPress::special(Key::Up)], "move-up");
        insert.bind(vec![KeyPress::special(Key::Right)], "move-right");
        // LSP completion navigation (Tab/Ctrl-n/Ctrl-p handled specially in binary
        // so they can either trigger/navigate the popup or fall through to Tab insert).
        // We bind them here so dispatch_builtin can route them.
        insert.bind(vec![KeyPress::special(Key::Tab)], "lsp-accept-completion");
        insert.bind(vec![KeyPress::ctrl('n')], "lsp-complete-next");
        insert.bind(vec![KeyPress::ctrl('p')], "lsp-complete-prev");
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
        visual.bind(parse_key_seq("^"), "move-to-first-non-blank");
        visual.bind(parse_key_seq("_"), "move-to-first-non-blank");
        visual.bind(parse_key_seq("+"), "move-line-next-non-blank");
        visual.bind(parse_key_seq("-"), "move-line-prev-non-blank");
        visual.bind(parse_key_seq("G"), "move-to-last-line");
        visual.bind(parse_key_seq("gg"), "move-to-first-line");
        visual.bind(parse_key_seq("w"), "move-word-forward");
        visual.bind(parse_key_seq("b"), "move-word-backward");
        visual.bind(parse_key_seq("e"), "move-word-end");
        visual.bind(parse_key_seq("W"), "move-big-word-forward");
        visual.bind(parse_key_seq("B"), "move-big-word-backward");
        visual.bind(parse_key_seq("E"), "move-big-word-end");
        visual.bind(parse_key_seq("ge"), "move-word-end-backward");
        visual.bind(parse_key_seq("gE"), "move-big-word-end-backward");
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
        visual.bind(parse_key_seq(">"), "visual-indent");
        visual.bind(parse_key_seq("<"), "visual-dedent");
        visual.bind(parse_key_seq("J"), "visual-join");
        visual.bind(parse_key_seq("p"), "visual-paste");
        visual.bind(parse_key_seq("P"), "visual-paste");
        visual.bind(parse_key_seq("o"), "visual-swap-ends");
        visual.bind(parse_key_seq("u"), "visual-lowercase");
        visual.bind(parse_key_seq("U"), "visual-uppercase");
        // Repeat find in visual mode
        visual.bind(parse_key_seq(";"), "repeat-find");
        visual.bind(parse_key_seq(","), "repeat-find-reverse");
        // Register prompt (same as Normal: `"<char>` routes the next op).
        visual.bind(parse_key_seq("\""), "prompt-register");
        // Surround visual selection: `S<char>`.
        visual.bind(parse_key_seq("S"), "surround-visual-await");
        // Text objects in visual mode
        visual.bind(parse_key_seq("i"), "visual-inner-object");
        visual.bind(parse_key_seq("a"), "visual-around-object");
        // Mode switches
        visual.bind(parse_key_seq("v"), "enter-visual-char");
        visual.bind(parse_key_seq("V"), "enter-visual-line");
        visual.bind(vec![KeyPress::special(Key::Escape)], "enter-normal-mode");
        // Tree-sitter structural expansion (Phase 4b M3)
        visual.bind(parse_key_seq_spaced("SPC s n"), "syntax-select-node");
        visual.bind(parse_key_seq_spaced("SPC s e"), "syntax-expand-selection");
        visual.bind(parse_key_seq_spaced("SPC s c"), "syntax-contract-selection");
        // Scheme eval region
        visual.bind(parse_key_seq_spaced("SPC e r"), "eval-region");
        visual.bind(parse_key_seq_spaced("SPC e S"), "send-region-to-shell");

        // Shell-insert keymap: minimal — only the escape sequence is bound.
        // All other keys are forwarded to the PTY by the main loop.
        // Users can add custom bindings via (define-key "shell-insert" ...).
        let mut shell_insert = Keymap::new("shell-insert");
        shell_insert.bind(parse_key_seq_spaced("C-\\ C-n"), "shell-normal-mode");

        // Git status keymap (Magit-lite)
        let mut git_status = Keymap::new("git-status");
        git_status.bind(parse_key_seq("j"), "move-down");
        git_status.bind(parse_key_seq("k"), "move-up");
        git_status.bind(parse_key_seq("s"), "git-stage");
        git_status.bind(parse_key_seq("u"), "git-unstage");
        git_status.bind(parse_key_seq("S"), "git-stage-all");
        git_status.bind(parse_key_seq("U"), "git-unstage-all");
        git_status.bind(parse_key_seq("c c"), "git-commit");
        git_status.bind(parse_key_seq("l l"), "git-log");
        git_status.bind(vec![KeyPress::special(Key::Tab)], "git-status-toggle");
        git_status.bind(vec![KeyPress::special(Key::Enter)], "git-status-open");
        git_status.bind(parse_key_seq("q"), "enter-normal-mode");
        git_status.bind(parse_key_seq("g r"), "git-status"); // Refresh

        // Org-mode keymap
        let mut org = Keymap::new("org");
        org.bind(vec![KeyPress::special(Key::Tab)], "org-cycle");
        org.bind(parse_key_seq_spaced("S-Left"), "org-todo-prev");
        org.bind(parse_key_seq_spaced("S-Right"), "org-todo-next");
        org.bind(parse_key_seq_spaced("S-Up"), "org-priority-up");
        org.bind(parse_key_seq_spaced("S-Down"), "org-priority-down");
        org.bind(vec![KeyPress::special(Key::Enter)], "org-open-link");

        maps.insert("normal".to_string(), normal);
        maps.insert("insert".to_string(), insert);
        maps.insert("visual".to_string(), visual);
        maps.insert("command".to_string(), Keymap::new("command"));
        maps.insert("shell-insert".to_string(), shell_insert);
        maps.insert("git-status".to_string(), git_status);
        maps.insert("org".to_string(), org);

        maps
    }
}
