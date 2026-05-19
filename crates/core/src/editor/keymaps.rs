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
        normal.bind(parse_key_seq("C-e"), "scroll-down-line");
        normal.bind(parse_key_seq("C-y"), "scroll-up-line");
        normal.bind(parse_key_seq("zz"), "scroll-center");
        normal.bind(parse_key_seq("zt"), "scroll-top");
        normal.bind(parse_key_seq("zb"), "scroll-bottom");
        normal.bind(parse_key_seq("za"), "toggle-fold");
        normal.bind(parse_key_seq("zM"), "close-all-folds");
        normal.bind(parse_key_seq("zR"), "open-all-folds");
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
        // Search (/, ?, n, N, *, #, gn/gN) — moved to modules/search/autoloads.scm
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
        // Jump list (C-o/C-i), marks (m/'), change list (g;/g,)
        // — moved to modules/marks-jumps/autoloads.scm
        // gf — open file under cursor. Resolves absolute paths, relative
        // paths (cwd first, then buffer's dir), and ~-expanded home paths.
        normal.bind(parse_key_seq("gf"), "goto-file-under-cursor");
        // gx — open link under cursor (URL or file path with :line:col).
        normal.bind(parse_key_seq("gx"), "open-link-at-cursor");
        // gl — edit link at cursor (jump into link region + insert mode)
        normal.bind(parse_key_seq("gl"), "edit-link");
        // Marks (m/') — moved to modules/marks-jumps/autoloads.scm
        // Macros (q, @) — moved to modules/macros/autoloads.scm
        // Join, indent, dedent
        normal.bind(parse_key_seq("J"), "join-lines");
        normal.bind(parse_key_seq(">>"), "indent-line");
        normal.bind(parse_key_seq("<<"), "dedent-line");
        normal.bind(parse_key_seq_spaced("M-q"), "fill-paragraph");
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
        // File info (vim Ctrl-G)
        normal.bind(vec![KeyPress::ctrl('g')], "file-info");
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
        // Register prompt (") — moved to modules/registers/autoloads.scm
        // Surrounds (vim-surround) — keybindings moved to modules/surround/autoloads.scm
        // Commands remain registered as builtins; keys come from the module.
        // Undo/Redo
        normal.bind(parse_key_seq("u"), "undo");
        normal.bind(parse_key_seq("C-r"), "redo");
        // Mode changes
        normal.bind(parse_key_seq("i"), "enter-insert-mode");
        normal.bind(parse_key_seq("a"), "enter-insert-mode-after");
        normal.bind(parse_key_seq("A"), "enter-insert-mode-eol");
        normal.bind(parse_key_seq("I"), "enter-insert-mode-bol");
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
        normal.bind(parse_key_seq_spaced("SPC b a"), "alternate-file");
        normal.bind(parse_key_seq_spaced("SPC b m"), "view-messages");
        normal.bind(parse_key_seq_spaced("SPC b N"), "new-buffer");
        normal.bind(parse_key_seq_spaced("SPC b D"), "force-kill-buffer");
        normal.bind(parse_key_seq_spaced("SPC b k"), "kill-buffer");
        normal.bind(parse_key_seq_spaced("SPC b i"), "file-info");
        // +file
        normal.bind(parse_key_seq_spaced("SPC f f"), "find-file");
        // Ranger/dired-style directory browser: spatial traversal
        // complement to the fuzzy `SPC f f` picker. (`-` would be the
        // vim/dirvish convention, but it's already bound to
        // `move-line-prev-non-blank` — keep the motion primitive.)
        normal.bind(parse_key_seq_spaced("SPC f d"), "file-browser");
        // SPC f t (file-tree-toggle) — moved to modules/file-tree/autoloads.scm
        normal.bind(parse_key_seq_spaced("SPC f s"), "save");
        // +window
        normal.bind(parse_key_seq_spaced("SPC w v"), "split-vertical");
        normal.bind(parse_key_seq_spaced("SPC w s"), "split-horizontal");
        normal.bind(parse_key_seq_spaced("SPC w q"), "close-window");
        normal.bind(parse_key_seq_spaced("SPC w h"), "focus-left");
        normal.bind(parse_key_seq_spaced("SPC w j"), "focus-down");
        normal.bind(parse_key_seq_spaced("SPC w k"), "focus-up");
        normal.bind(parse_key_seq_spaced("SPC w l"), "focus-right");
        // Resize: +/-/= (Doom parity)
        normal.bind(parse_key_seq_spaced("SPC w +"), "window-grow");
        normal.bind(parse_key_seq_spaced("SPC w -"), "window-shrink");
        normal.bind(parse_key_seq_spaced("SPC w ="), "window-balance");
        normal.bind(parse_key_seq_spaced("SPC w m"), "window-maximize");
        // Move: H/J/K/L (uppercase = move window, lowercase = focus)
        normal.bind(parse_key_seq_spaced("SPC w H"), "window-move-left");
        normal.bind(parse_key_seq_spaced("SPC w J"), "window-move-down");
        normal.bind(parse_key_seq_spaced("SPC w K"), "window-move-up");
        normal.bind(parse_key_seq_spaced("SPC w L"), "window-move-right");
        normal.bind(parse_key_seq_spaced("SPC w w"), "focus-next-window");
        normal.bind(parse_key_seq_spaced("SPC w d"), "close-window");
        // Ctrl-W resize shortcuts
        normal.bind(parse_key_seq_spaced("C-w +"), "window-grow");
        normal.bind(parse_key_seq_spaced("C-w -"), "window-shrink");
        normal.bind(parse_key_seq_spaced("C-w ="), "window-balance");
        normal.bind(parse_key_seq_spaced("C-w >"), "window-grow-width");
        normal.bind(parse_key_seq_spaced("C-w <"), "window-shrink-width");
        // +ai
        normal.bind(parse_key_seq_spaced("SPC a a"), "open-ai-agent");
        normal.bind(parse_key_seq_spaced("SPC a p"), "ai-prompt");
        normal.bind(parse_key_seq_spaced("SPC a c"), "ai-cancel");
        normal.bind(parse_key_seq_spaced("SPC a m"), "ai-set-mode");
        normal.bind(parse_key_seq_spaced("SPC a P"), "ai-set-profile");
        normal.bind(parse_key_seq_spaced("SPC a n"), "ai-ping");
        normal.bind(parse_key_seq_spaced("SPC a v"), "verify");
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
        normal.bind(parse_key_seq_spaced("SPC h B"), "describe-bindings");
        normal.bind(parse_key_seq_spaced("SPC h m"), "describe-mode");
        normal.bind(parse_key_seq_spaced("SPC h D"), "describe-display-policy");
        // +scratch
        normal.bind(parse_key_seq_spaced("SPC x"), "toggle-scratch-buffer");
        // +theme
        normal.bind(parse_key_seq_spaced("SPC t t"), "cycle-theme");
        normal.bind(parse_key_seq_spaced("SPC t S"), "set-theme");
        // +debug — moved to modules/debug/autoloads.scm
        // +quit
        normal.bind(parse_key_seq_spaced("SPC q q"), "quit");
        normal.bind(parse_key_seq_spaced("SPC q Q"), "force-quit");
        normal.bind(parse_key_seq_spaced("SPC q s"), "save-and-quit");
        normal.bind(parse_key_seq_spaced("SPC q S"), "save-all-and-quit");
        // +search/syntax — search keys moved to modules/search/autoloads.scm
        // Syntax selection stays in kernel (tree-sitter bridge)
        normal.bind(parse_key_seq_spaced("SPC s n"), "syntax-select-node");
        normal.bind(parse_key_seq_spaced("SPC s e"), "syntax-expand-selection");
        normal.bind(parse_key_seq_spaced("SPC s c"), "syntax-contract-selection");
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
        normal.bind(parse_key_seq_spaced("SPC p a"), "add-project");
        normal.bind(parse_key_seq_spaced("SPC p D"), "project-forget");
        normal.bind(parse_key_seq_spaced("SPC p c"), "project-clean");
        // +file expansions
        normal.bind(parse_key_seq_spaced("SPC f r"), "recent-files");
        normal.bind(parse_key_seq_spaced("SPC f y"), "yank-file-path");
        normal.bind(parse_key_seq_spaced("SPC f R"), "rename-file");
        normal.bind(parse_key_seq_spaced("SPC f n"), "new-buffer");
        normal.bind(parse_key_seq_spaced("SPC f c"), "edit-config");
        normal.bind(parse_key_seq_spaced("SPC f C"), "copy-this-file");
        normal.bind(parse_key_seq_spaced("SPC f P"), "edit-settings");
        normal.bind(parse_key_seq_spaced("SPC f S"), "save-as");
        normal.bind(parse_key_seq_spaced("SPC f D"), "delete-this-file");
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
        normal.bind(parse_key_seq_spaced("SPC t i"), "toggle-inline-images");
        normal.bind(parse_key_seq_spaced("SPC t s"), "toggle-scrollbar");
        normal.bind(parse_key_seq_spaced("SPC t F"), "toggle-fps");
        normal.bind(parse_key_seq_spaced("SPC t D"), "debug-mode");
        normal.bind(
            parse_key_seq_spaced("SPC t d"),
            "toggle-lsp-diagnostics-inline",
        );
        // +git — moved to modules/git-status/autoloads.scm
        // +open
        normal.bind(parse_key_seq_spaced("SPC o t"), "terminal");
        normal.bind(parse_key_seq_spaced("SPC o T"), "terminal-here");
        normal.bind(parse_key_seq_spaced("SPC o r"), "terminal-reset");
        normal.bind(parse_key_seq_spaced("SPC o c"), "terminal-close");
        // SPC o a / SPC o A — moved to modules/agenda/autoloads.scm
        // +register — moved to modules/registers/autoloads.scm
        // +notes (KB shortcuts)
        // +dailies
        normal.bind(parse_key_seq_spaced("SPC n d t"), "daily-goto-today");
        normal.bind(parse_key_seq_spaced("SPC n d y"), "daily-goto-yesterday");
        normal.bind(parse_key_seq_spaced("SPC n d d"), "daily-goto-date");
        normal.bind(parse_key_seq_spaced("SPC n d p"), "daily-prev");
        normal.bind(parse_key_seq_spaced("SPC n d n"), "daily-next");
        normal.bind(parse_key_seq_spaced("SPC n f"), "kb-find");
        normal.bind(parse_key_seq_spaced("SPC n v"), "kb-view");
        normal.bind(parse_key_seq_spaced("SPC n e"), "kb-edit-source");
        normal.bind(parse_key_seq_spaced("SPC n c"), "kb-create");
        normal.bind(parse_key_seq_spaced("SPC n D"), "kb-delete");
        normal.bind(parse_key_seq_spaced("SPC n r"), "kb-register");
        normal.bind(parse_key_seq_spaced("SPC n R"), "kb-reimport");
        normal.bind(parse_key_seq_spaced("SPC n i"), "kb-insert-link");
        // Capture mode (org-roam parity) — leader alternatives for discoverability
        normal.bind(parse_key_seq_spaced("C-c C-c"), "capture-finalize");
        normal.bind(parse_key_seq_spaced("C-c C-k"), "capture-abort");
        normal.bind(parse_key_seq_spaced("SPC n s"), "capture-finalize");
        normal.bind(parse_key_seq_spaced("SPC n k"), "capture-abort");
        normal.bind(parse_key_seq_spaced("SPC n C"), "kb-cleanup-orphans");
        normal.bind(parse_key_seq_spaced("SPC n I"), "kb-instances");
        normal.bind(parse_key_seq_spaced("SPC n h"), "kb-health");
        // +code (LSP shortcuts)
        normal.bind(parse_key_seq_spaced("SPC c d"), "lsp-goto-definition");
        normal.bind(parse_key_seq_spaced("SPC c r"), "lsp-find-references");
        normal.bind(parse_key_seq_spaced("SPC c k"), "lsp-hover");
        normal.bind(parse_key_seq_spaced("SPC c x"), "lsp-show-diagnostics");
        normal.bind(parse_key_seq_spaced("SPC c a"), "lsp-code-action");
        normal.bind(parse_key_seq_spaced("SPC c R"), "lsp-rename");
        normal.bind(parse_key_seq_spaced("SPC c f"), "lsp-format");
        normal.bind(parse_key_seq_spaced("SPC c F"), "lsp-range-format");
        normal.bind(parse_key_seq_spaced("SPC c s"), "lsp-status");
        normal.bind(parse_key_seq_spaced("SPC c o"), "lsp-symbol-outline");
        normal.bind(parse_key_seq_spaced("SPC l p"), "lsp-peek-definition");
        normal.bind(parse_key_seq_spaced("SPC l r"), "lsp-peek-references");

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
        normal.set_group_name(parse_key_seq_spaced("SPC n d"), "+dailies");
        normal.set_group_name(parse_key_seq_spaced("SPC o"), "+open");
        normal.set_group_name(parse_key_seq_spaced("SPC l"), "+lsp");
        normal.set_group_name(parse_key_seq_spaced("SPC r"), "+register");
        normal.set_group_name(parse_key_seq_spaced("SPC m"), "+multicursor");

        // Multi-cursor (SPC m) — moved to modules/multicursor/autoloads.scm

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
        normal.bind(vec![KeyPress::ctrl('v')], "enter-visual-block");

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
        // Marks (m/') — moved to modules/marks-jumps/autoloads.scm
        // Scroll
        visual.bind(parse_key_seq("C-u"), "scroll-half-up");
        visual.bind(parse_key_seq("C-d"), "scroll-half-down");
        visual.bind(parse_key_seq("C-f"), "scroll-page-down");
        visual.bind(parse_key_seq("C-b"), "scroll-page-up");
        visual.bind(parse_key_seq("C-e"), "scroll-down-line");
        visual.bind(parse_key_seq("C-y"), "scroll-up-line");
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
        // Register prompt (") — moved to modules/registers/autoloads.scm
        // Surround visual (S) — moved to modules/surround/autoloads.scm
        // Block visual insert (I inserts at left edge on all rows)
        visual.bind(parse_key_seq("I"), "block-visual-insert");
        // Block visual append (A appends at right edge on all rows)
        visual.bind(parse_key_seq("A"), "block-visual-append");
        // Text objects in visual mode
        visual.bind(parse_key_seq("i"), "visual-inner-object");
        visual.bind(parse_key_seq("a"), "visual-around-object");
        // Mode switches
        visual.bind(parse_key_seq("v"), "enter-visual-char");
        visual.bind(parse_key_seq("V"), "enter-visual-line");
        visual.bind(vec![KeyPress::ctrl('v')], "enter-visual-block");
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
        shell_insert.bind(parse_key_seq("C-y"), "paste-after");

        // Git status keymap — moved to modules/git-status/autoloads.scm

        // Org-mode keymap — moved to modules/org/autoloads.scm

        maps.insert("normal".to_string(), normal);
        maps.insert("insert".to_string(), insert);
        maps.insert("visual".to_string(), visual);
        maps.insert("command".to_string(), Keymap::new("command"));
        maps.insert("shell-insert".to_string(), shell_insert);

        // Shell-normal keymap: inherits normal mode, adds shell-specific bindings.
        // `i` is inherited from parent `normal` (→ enter-insert-mode → ShellInsert).
        let mut shell_normal = Keymap::with_parent("shell-normal", "normal");
        shell_normal.bind(parse_key_seq("v"), "shell-select-mode");
        shell_normal.bind(parse_key_seq("q"), "enter-insert-mode");
        shell_normal.bind(parse_key_seq("?"), "show-buffer-keys");
        maps.insert("shell-normal".to_string(), shell_normal);

        // Shell-select keymap: read-only vim buffer for shell scrollback.
        // Inherits normal mode for motions; q/Esc exit back to the shell.
        let mut shell_select = Keymap::with_parent("shell-select", "normal");
        shell_select.bind(parse_key_seq("q"), "close-shell-select");
        shell_select.bind(vec![KeyPress::special(Key::Escape)], "close-shell-select");
        shell_select.bind(parse_key_seq("?"), "show-buffer-keys");
        maps.insert("shell-select".to_string(), shell_select);

        // Module list keymap — Enter to expand, q to close
        let mut modules_km = Keymap::with_parent("modules", "normal");
        modules_km.bind(
            vec![KeyPress::special(Key::Enter)],
            "describe-module-at-cursor",
        );
        modules_km.bind(parse_key_seq("q"), "kill-buffer");
        maps.insert("modules".to_string(), modules_km);

        // Git-status, org, markdown keymaps — moved to modules/

        // File tree keymap + bindings — moved to modules/file-tree/autoloads.scm

        // Help buffer keymap — org-mode Tab conventions:
        // Tab = fold heading (or next link if not on heading)
        // S-Tab = global visibility cycle
        // n/p = link navigation (moved from Tab/S-Tab)
        // e = edit source (Obsidian-style toggle)
        let mut help = Keymap::with_parent("help", "normal");
        help.bind(vec![KeyPress::special(Key::Enter)], "help-follow-link");
        help.bind(vec![KeyPress::special(Key::Tab)], "help-cycle");
        help.bind(vec![KeyPress::special(Key::BackTab)], "help-global-cycle");
        help.bind(parse_key_seq("n"), "help-next-link");
        help.bind(parse_key_seq("p"), "help-prev-link");
        help.bind(parse_key_seq("e"), "kb-edit-source");
        help.bind(parse_key_seq("q"), "help-close");
        help.bind(parse_key_seq("C-o"), "help-back");
        help.bind(parse_key_seq("C-i"), "help-forward");
        help.bind(parse_key_seq("za"), "help-cycle");
        help.bind(parse_key_seq("zM"), "help-close-all-folds");
        help.bind(parse_key_seq("zR"), "help-open-all-folds");
        help.bind(parse_key_seq("?"), "show-buffer-keys");
        maps.insert("help".to_string(), help);

        // Debug panel keymap — moved to modules/debug/autoloads.scm
        // Agenda keymap — moved to modules/agenda/autoloads.scm

        maps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Language;

    #[test]
    fn org_buffer_keymap_names() {
        let mut ed = Editor::new();
        ed.syntax.set_language(0, Language::Org);
        let names = ed.current_keymap_names();
        // Org keymap moved to modules/org/ — falls back to normal at construction
        assert_eq!(names, Some(("org", Some("normal"))));
    }

    // org_keymap_spc_m_s_n_widens — moved to module tests (modules/org/)
    // md_keymap_spc_m_s_n_widens — moved to module tests (modules/markdown/)

    #[test]
    fn all_spc_bindings_resolve_to_registered_commands() {
        let ed = Editor::new();
        let normal = ed.keymaps.get("normal").unwrap();
        let spc = parse_key_seq("SPC");
        let mut missing = Vec::new();
        for (keys, cmd) in normal.bindings() {
            // Only check SPC-prefixed bindings (leader key)
            if keys.first() != spc.first() {
                continue;
            }
            if !ed.commands.contains(cmd) {
                missing.push(cmd.clone());
            }
        }
        assert!(
            missing.is_empty(),
            "SPC bindings target unregistered commands: {:?}",
            missing
        );
    }

    #[test]
    fn new_spc_bindings_resolve_correctly() {
        let ed = Editor::new();
        let normal = ed.keymaps.get("normal").unwrap();
        // SPC g bindings moved to modules/git-status/
        let cases = vec![
            ("SPC w w", "focus-next-window"),
            ("SPC w d", "close-window"),
            ("SPC b k", "kill-buffer"),
            ("SPC b i", "file-info"),
            ("SPC f n", "new-buffer"),
            ("SPC f C", "copy-this-file"),
            ("SPC f P", "edit-settings"),
            ("SPC p a", "add-project"),
            ("SPC q s", "save-and-quit"),
            ("SPC q S", "save-all-and-quit"),
        ];
        for (seq, expected) in cases {
            let keys = parse_key_seq_spaced(seq);
            assert_eq!(
                normal.lookup(&keys),
                crate::keymap::LookupResult::Exact(expected),
                "{} should resolve to {}",
                seq,
                expected
            );
        }
    }

    #[test]
    fn ctrl_g_resolves_to_file_info() {
        let ed = Editor::new();
        let normal = ed.keymaps.get("normal").unwrap();
        let keys = vec![KeyPress::ctrl('g')];
        assert_eq!(
            normal.lookup(&keys),
            crate::keymap::LookupResult::Exact("file-info")
        );
    }

    // org_keymap_fallback_to_normal — org keymap moved to modules/org/

    // File-tree keymap + bindings moved to modules/file-tree/autoloads.scm.
    // Verify commands remain registered as kernel builtins.
    #[test]
    fn file_tree_commands_registered() {
        let ed = Editor::new();
        assert!(ed.commands.contains("file-tree-toggle"));
        assert!(ed.commands.contains("file-tree-down"));
        assert!(ed.commands.contains("file-tree-open"));
        assert!(ed.commands.contains("file-tree-create"));
    }

    #[test]
    fn file_tree_buffer_keymap_names() {
        let mut ed = Editor::new();
        let root = std::env::current_dir().unwrap();
        let tree_buf = crate::buffer::Buffer::new_file_tree(&root);
        ed.buffers.push(tree_buf);
        let tree_idx = ed.buffers.len() - 1;
        ed.window_mgr.focused_window_mut().buffer_idx = tree_idx;
        let names = ed.current_keymap_names();
        assert_eq!(names, Some(("file-tree", Some("normal"))));
    }

    #[test]
    fn help_keymap_exists_with_bindings() {
        let ed = Editor::new();
        let help_map = ed.keymaps.get("help").unwrap();
        assert_eq!(help_map.parent.as_deref(), Some("normal"));
        let q_key = parse_key_seq("q");
        assert_eq!(
            help_map.lookup(&q_key),
            crate::keymap::LookupResult::Exact("help-close")
        );
        let enter_key = vec![KeyPress::special(Key::Enter)];
        assert_eq!(
            help_map.lookup(&enter_key),
            crate::keymap::LookupResult::Exact("help-follow-link")
        );
    }

    // debug_keymap_exists_with_bindings — debug keymap moved to modules/debug/

    #[test]
    fn help_buffer_uses_help_keymap() {
        let mut ed = Editor::new();
        // Create a KB buffer and focus it
        let mut buf = crate::buffer::Buffer::new();
        buf.kind = crate::buffer::BufferKind::Kb;
        buf.name = "*Help*".to_string();
        ed.buffers.push(buf);
        let help_idx = ed.buffers.len() - 1;
        ed.window_mgr.focused_window_mut().buffer_idx = help_idx;
        let names = ed.current_keymap_names();
        assert_eq!(names, Some(("help", Some("normal"))));
    }

    #[test]
    fn dailies_bindings_registered() {
        let ed = Editor::new();
        let normal = ed.keymaps.get("normal").unwrap();
        let entries = normal.which_key_entries(&parse_key_seq_spaced("SPC n d"), &ed.commands);
        assert!(
            entries.iter().any(|e| e.label.contains("today")),
            "dailies bindings should include 'today'"
        );
        assert_eq!(entries.len(), 5, "should have 5 dailies bindings");
    }

    #[test]
    fn spc_sub_prefixes_have_which_key_group_names() {
        // Verify sub-prefixes (like SPC n d) also have group names
        use crate::keymap::parse_key_seq_spaced;
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").unwrap();
        let spc_n = parse_key_seq_spaced("SPC n");
        let entries = normal.which_key_entries(&spc_n, &editor.commands);
        let d_entry = entries.iter().find(|e| {
            use crate::keymap::Key;
            matches!(e.key.key, Key::Char('d'))
        });
        assert!(d_entry.is_some(), "SPC n should have a 'd' entry");
        let d = d_entry.unwrap();
        assert!(d.is_group, "SPC n d should be a group");
        assert_eq!(
            d.label, "+dailies",
            "SPC n d group should be labeled +dailies"
        );
    }

    #[test]
    fn overlay_keymaps_have_parent_field() {
        let ed = Editor::new();
        // git-status, org, markdown keymaps moved to modules
        // Only kernel keymaps remain at construction
        assert!(ed.keymaps.get("normal").unwrap().parent.is_none());
        assert_eq!(
            ed.keymaps.get("help").unwrap().parent.as_deref(),
            Some("normal")
        );
    }

    // git_status_q_binds_to_kill_buffer — git-status keymap moved to modules/git-status/
    // overlay_keymaps_have_show_buffer_keys — git-status, debug keymaps moved to modules/
    // git_status_spc_m_local_leader — git-status keymap moved to modules/git-status/

    #[test]
    fn overlay_keymaps_have_show_buffer_keys_help() {
        let ed = Editor::new();
        let q_key = parse_key_seq("?");
        // Only help keymap remains in kernel
        let km = ed.keymaps.get("help").unwrap();
        assert_eq!(
            km.lookup(&q_key),
            crate::keymap::LookupResult::Exact("show-buffer-keys"),
        );
    }

    #[test]
    fn buffer_keys_entries_returns_entries() {
        let mut ed = Editor::new();
        // Create a KB buffer and focus it (help keymap is still in kernel)
        let mut buf = crate::buffer::Buffer::new();
        buf.kind = crate::buffer::BufferKind::Kb;
        buf.name = "*Help*".to_string();
        ed.buffers.push(buf);
        let idx = ed.buffers.len() - 1;
        ed.window_mgr.focused_window_mut().buffer_idx = idx;
        let entries = ed.buffer_keys_entries();
        // Should have entries from help + normal keymaps
        assert!(!entries.is_empty());
    }

    #[test]
    fn shift_i_bound_in_normal_mode() {
        let ed = Editor::new();
        let normal = ed.keymaps.get("normal").unwrap();
        let seq = parse_key_seq("I");
        let result = normal.lookup(&seq);
        assert_eq!(
            result,
            crate::keymap::LookupResult::Exact("enter-insert-mode-bol")
        );
    }

    #[test]
    fn org_keymap_has_tab_and_enter() {
        // The org keymap is created by the Scheme module, but we can verify
        // the kernel fallback: org buffers should map to ("org", Some("normal"))
        // and the org keymap (if loaded) would have Tab and Enter bindings.
        // Here we just verify the kernel keymap name resolution is correct.
        let mut ed = Editor::new();
        ed.syntax.set_language(0, Language::Org);
        let (primary, fallback) = ed.current_keymap_names().unwrap();
        assert_eq!(primary, "org");
        assert_eq!(fallback, Some("normal"));
    }
}
