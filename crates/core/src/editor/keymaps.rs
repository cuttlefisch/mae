use std::collections::HashMap;

use crate::keymap::{parse_key_seq, parse_key_seq_spaced, Key, KeyPress, Keymap, WhichKeyEntry};
use crate::Mode;

use super::Editor;

impl Editor {
    /// Create the default vi-like keymaps.
    pub fn default_keymaps() -> HashMap<String, Keymap> {
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

        // ── Leader (SPC) tree: single source of truth in the keymap flavor ──
        // The full SPC leader tree (buffer/file/window/ai/help/project/notes/
        // code/eval/toggle/open/collab/…) lives in the keymap flavor module
        // (modules/keymap-doom/autoloads.scm), embedded in the binary and loaded
        // at startup (bootstrap::load_modules → builtin_module_dirs + embedded
        // baseline). The kernel deliberately keeps ONLY vi-modal primitives so
        // there is no second, drifting copy — the duplicated kernel tree had
        // already fallen out of sync (e.g. SPC C collaboration was module-only).
        // Override or extend any leader binding at runtime via (define-key ...)
        // or :reload-modules; select a different flavor via the keymap_flavor
        // option. See ADR / RENDERING-vs-keymap notes in CLAUDE.md.

        // Ctrl-W window-resize primitives (vim-style), complementing the C-w
        // split/focus bindings above. These are kernel primitives, not leader
        // bindings.
        normal.bind(parse_key_seq_spaced("C-w +"), "window-grow");
        normal.bind(parse_key_seq_spaced("C-w -"), "window-shrink");
        normal.bind(parse_key_seq_spaced("C-w ="), "window-balance");
        normal.bind(parse_key_seq_spaced("C-w >"), "window-grow-width");
        normal.bind(parse_key_seq_spaced("C-w <"), "window-shrink-width");

        // Capture (org-roam parity) — standalone Emacs-style chords, not part of
        // the leader tree, so they stay in the kernel.
        normal.bind(parse_key_seq_spaced("C-c C-c"), "capture-finalize");
        normal.bind(parse_key_seq_spaced("C-c C-k"), "capture-abort");

        // Escape in Normal mode: previously unbound entirely, which meant it
        // was silently swallowed as unbound input. `dispatch_builtin_inner`
        // auto-dismisses several transient popups (LSP hover, KB-link
        // preview, signature help, peek-definition, code-action menu) as a
        // side effect of dispatching ANY command not on that popup's own
        // exclusion list — but an UNBOUND key never reaches dispatch at all,
        // so those popups had no way to be dismissed via Escape (only
        // incidentally, by happening to press some other bound key).
        // "enter-normal-mode" is already the Escape target in Insert/Visual
        // modes and is a proven no-op when already in Normal mode
        // (`set_mode` no-ops and skips the mode-change hook when the target
        // mode matches the current one) — reusing it here needs no new
        // command, just gets Escape flowing through the same dispatch path
        // that already knows how to dismiss every one of those popups.
        normal.bind(vec![KeyPress::special(Key::Escape)], "enter-normal-mode");

        let mut insert = Keymap::new("insert");
        insert.bind(vec![KeyPress::special(Key::Escape)], "enter-normal-mode");
        insert.bind(vec![KeyPress::special(Key::Left)], "move-left");
        insert.bind(vec![KeyPress::special(Key::Down)], "move-down");
        insert.bind(vec![KeyPress::special(Key::Up)], "move-up");
        insert.bind(vec![KeyPress::special(Key::Right)], "move-right");
        // LSP completion navigation (Tab/Ctrl-n/Ctrl-p handled specially in binary
        // so they can either trigger/navigate the popup or fall through to Tab insert).
        // Tab is owned by the snippet module (snippet-expand-or-next), with fallback
        // to lsp-accept-completion in keymap-doom if snippets are not loaded.
        // Binary insert.rs handles Tab directly via pattern match before keymap dispatch.
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
        // Visual-mode SPC leader bindings (tree-sitter structural expansion,
        // Scheme eval-region, …) live in the keymap flavor module
        // (modules/keymap-doom/autoloads.scm), alongside the normal-mode tree —
        // single source of truth, no kernel duplication.

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
        // The shared leader-key tree (populated by the keymap-leader module +
        // feature modules; consulted by the transient keypad when leader_active).
        // Created empty in the kernel so any module can bind into it regardless
        // of load order, and so it survives reset_keymaps_to_kernel.
        maps.insert("leader".to_string(), Keymap::new("leader"));
        // Mode-local-leader trees (parent `leader`). The transient keypad consults
        // `<mode>-leader` BEFORE the global leader in that mode's buffers, so
        // `SPC m` is a major-mode local leader (org babel/export, markdown, table
        // editing) WITHOUT the mode binding `SPC …` into its own keymap — which
        // used to shadow `SPC → leader-dispatch` and leave only `m` visible.
        // Kernel-created (like `leader`) so org/markdown/tables can bind into them
        // regardless of load order and they survive reset_keymaps_to_kernel.
        maps.insert(
            "org-leader".to_string(),
            Keymap::with_parent("org-leader", "leader"),
        );
        maps.insert(
            "markdown-leader".to_string(),
            Keymap::with_parent("markdown-leader", "leader"),
        );
        // The shared `navigation` context keymap: flavor-independent movement and
        // leader access for read-only nav buffers (dashboard, file tree, modules,
        // help, …). Created empty in the kernel (parent `normal`) so it survives
        // reset_keymaps_to_kernel and any module can bind into it; populated by
        // the keymap-leader module. Nav overlays parent onto it (below) so the
        // resolution chain is overlay → navigation → normal.
        maps.insert(
            "navigation".to_string(),
            Keymap::with_parent("navigation", "normal"),
        );
        maps.insert("command".to_string(), Keymap::new("command"));
        maps.insert("shell-insert".to_string(), shell_insert);

        // Shell-normal keymap: inherits normal mode, adds shell-specific bindings.
        // `i` is inherited from parent `normal` (→ enter-insert-mode → ShellInsert).
        let mut shell_normal = Keymap::with_parent("shell-normal", "navigation");
        shell_normal.bind(parse_key_seq("v"), "shell-select-mode");
        shell_normal.bind(parse_key_seq("q"), "enter-insert-mode");
        shell_normal.bind(parse_key_seq("?"), "show-buffer-keys");
        maps.insert("shell-normal".to_string(), shell_normal);

        // Shell-select keymap: read-only vim buffer for shell scrollback.
        // Inherits normal mode for motions; q/Esc exit back to the shell.
        let mut shell_select = Keymap::with_parent("shell-select", "navigation");
        shell_select.bind(parse_key_seq("q"), "close-shell-select");
        shell_select.bind(vec![KeyPress::special(Key::Escape)], "close-shell-select");
        shell_select.bind(parse_key_seq("?"), "show-buffer-keys");
        maps.insert("shell-select".to_string(), shell_select);

        // Module list keymap — Enter to expand, q to close
        let mut modules_km = Keymap::with_parent("modules", "navigation");
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
        let mut help = Keymap::with_parent("help", "navigation");
        help.bind(vec![KeyPress::special(Key::Enter)], "help-follow-link");
        help.bind(vec![KeyPress::special(Key::Tab)], "help-cycle");
        help.bind(vec![KeyPress::special(Key::BackTab)], "help-global-cycle");
        help.bind(parse_key_seq("n"), "help-next-link");
        help.bind(parse_key_seq("p"), "help-prev-link");
        help.bind(parse_key_seq("e"), "kb-edit-source");
        help.bind(parse_key_seq("P"), "kb-promote");
        // KB-link hover preview (Part D) — mirrors normal mode's "K" ->
        // lsp-hover binding above, scoped to the KB buffer's own keymap.
        help.bind(parse_key_seq("K"), "kb-preview");
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

impl Editor {
    /// Returns the primary keymap name and optional fallback for the current mode.
    /// Buffer-kind overlays (git-status, file-tree, help, debug) and language
    /// overlays (org, markdown) sit on top of "normal" — if the overlay has no
    /// match, the caller should retry with the fallback.
    pub fn current_keymap_names(&self) -> Option<(&str, Option<&str>)> {
        // Transient keypad/leader layer overrides mode-based keymap selection:
        // while active, keys resolve against the shared `leader` keymap (the mae
        // which-key tree), regardless of the underlying mode (Normal for the doom
        // flavor, Insert for the non-modal flavor). See `Editor::leader_active`.
        if self.leader_active {
            // Mode-aware local leader: if the active buffer's context has a
            // local-leader keymap (which parents on `leader`), the keypad
            // consults it FIRST — so `SPC m` is the major-mode local leader (org
            // babel/export in an org buffer) while `SPC b/f/w/…` still fall
            // through to the global `leader`. Only used when the keymap actually
            // exists (a module opted in by creating it); otherwise the plain
            // global leader, exactly as before.
            let idx = self.active_buffer_idx();
            let kind = self.buffers[idx].kind;
            let local_leader = self
                .keymap_registry
                .local_leader_for_kind(kind)
                .or_else(|| {
                    self.syntax
                        .language_of(idx)
                        .and_then(|l| self.keymap_registry.local_leader_for_language(l))
                });
            if let Some(ll) = local_leader {
                if self.keymaps.contains_key(ll) {
                    return Some((ll, None));
                }
            }
            return Some(("leader", None));
        }

        let idx = self.active_buffer_idx();
        let kind = self.buffers[idx].kind;
        let lang = self.syntax.language_of(idx);

        match self.mode {
            Mode::Normal => {
                // Context keymap from the data-driven registry: buffer kind first
                // (git-status, file-tree, navigation, …), then language overlay
                // (org/markdown). Both fall back to "normal". No hardcoded match —
                // a module can route a new kind/language without a kernel patch.
                if let Some(km_name) = self.keymap_registry.context_for_kind(kind) {
                    Some((km_name, Some("normal")))
                } else if let Some(km_name) =
                    lang.and_then(|l| self.keymap_registry.context_for_language(l))
                {
                    Some((km_name, Some("normal")))
                } else {
                    Some(("normal", None))
                }
            }
            Mode::Insert => Some(("insert", None)),
            Mode::Visual(_) => Some(("visual", None)),
            Mode::Command
            | Mode::ConversationInput
            | Mode::Search
            | Mode::FilePicker
            | Mode::FileBrowser
            | Mode::CommandPalette => Some(("command", None)),
            Mode::ShellInsert => None,
        }
    }

    /// Get the keymap for the current mode.
    pub fn current_keymap(&self) -> Option<&Keymap> {
        let (name, _) = self.current_keymap_names()?;
        self.keymaps.get(name)
    }

    /// The ordered keymap resolution chain for the current focus, most-specific
    /// layer first.
    ///
    /// This is the SINGLE source of truth consumed by keystroke dispatch
    /// (`handle_keymap_mode`), the which-key popup (`merged_which_key_entries`),
    /// and `describe-bindings`. Routing all three through one chain makes the
    /// keymap a key *resolves against* and the keymap the UI *shows* incapable of
    /// diverging — previously dispatch used a flat `(primary, fallback)` pair
    /// while `describe-bindings` walked the `parent` chain N levels, so a 3-deep
    /// chain (e.g. `git-log → git-status → normal`) would dispatch and display
    /// differently.
    ///
    /// The chain is `current_keymap_names()`'s primary keymap plus its `parent`
    /// ancestry, followed by the fallback plus its ancestry (deduped, cycle-safe).
    /// For the current 2-deep keymaps this reproduces the old behavior exactly.
    /// Empty when there is no keymap (ShellInsert — keys go straight to the PTY).
    ///
    /// Phase 0 derives the chain from the existing `current_keymap_names()` match;
    /// a later phase replaces the source with the data-driven keymap registry
    /// without changing any consumer.
    pub fn keymap_chain(&self) -> Vec<String> {
        let Some((primary, fallback)) = self.current_keymap_names() else {
            return Vec::new();
        };
        let mut chain: Vec<String> = Vec::new();
        self.extend_keymap_chain(primary, &mut chain);
        if let Some(fb) = fallback {
            self.extend_keymap_chain(fb, &mut chain);
        }
        chain
    }

    /// Append `start` and its `parent` ancestry to `chain`, skipping any name
    /// already present (dedupe + cycle guard).
    fn extend_keymap_chain(&self, start: &str, chain: &mut Vec<String>) {
        let mut cur = Some(start.to_string());
        while let Some(name) = cur.take() {
            if chain.iter().any(|n| n == &name) {
                break;
            }
            cur = self.keymaps.get(&name).and_then(|km| km.parent.clone());
            chain.push(name);
        }
    }

    /// Reset all keymaps to the fresh kernel defaults (vi-modal primitives only,
    /// no leader tree). Used by runtime keymap-flavor switching: reset to a clean
    /// slate, then re-run module loading to apply the new flavor — avoids stale
    /// bindings from the previous flavor (the `leader`/`insert` entries differ).
    pub fn reset_keymaps_to_kernel(&mut self) {
        self.keymaps = Self::default_keymaps();
        // Re-seed the context routing to the kernel baseline too; module
        // registrations (e.g. a "navigation" context, canvas artifact) re-apply
        // on the subsequent module reload, exactly like the keymaps themselves.
        self.keymap_registry = crate::keymap_registry::KeymapRegistry::kernel_defaults();
        self.set_leader_active(false);
        self.clear_which_key_prefix();
    }

    /// Look up a key binding by key string (e.g. "SPC n d t").
    /// Returns (command_name, keymap_name) if found.
    pub fn lookup_key_binding(&self, key_str: &str) -> Option<(String, String)> {
        let seq = crate::keymap::parse_key_seq_spaced(key_str);
        if seq.is_empty() {
            return None;
        }
        for (name, km) in &self.keymaps {
            for (bound_seq, cmd) in km.bindings() {
                if *bound_seq == seq {
                    return Some((cmd.clone(), name.clone()));
                }
            }
        }
        None
    }

    /// Query keybindings across all keymaps with optional filters.
    /// Returns vec of (key_display, command, keymap_name).
    pub fn query_keybindings(
        &self,
        keymap_filter: Option<&str>,
        command_filter: Option<&str>,
        prefix_filter: Option<&str>,
    ) -> Vec<(String, String, String)> {
        let prefix_seq = prefix_filter.map(crate::keymap::parse_key_seq_spaced);
        let mut results = Vec::new();
        for (name, km) in &self.keymaps {
            if let Some(filter) = keymap_filter {
                if name != filter {
                    continue;
                }
            }
            for (seq, cmd) in km.bindings() {
                if let Some(ref cmd_filter) = command_filter {
                    if !cmd.contains(cmd_filter) {
                        continue;
                    }
                }
                if let Some(ref prefix) = prefix_seq {
                    if seq.len() < prefix.len() || &seq[..prefix.len()] != prefix.as_slice() {
                        continue;
                    }
                }
                let key_display = crate::keymap::format_key_seq(seq);
                results.push((key_display, cmd.clone(), name.clone()));
            }
        }
        results.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)));
        results
    }

    /// Merge which-key entries across the full resolution chain (most-specific
    /// layer first; a more-specific layer's binding for a key shadows a deeper
    /// one). Uses the same `keymap_chain()` as dispatch so the popup can't show a
    /// key the dispatcher wouldn't run.
    fn merged_which_key_entries(&self, prefix: &[KeyPress]) -> Vec<WhichKeyEntry> {
        let mut entries: Vec<WhichKeyEntry> = Vec::new();
        let mut existing: std::collections::HashSet<String> = std::collections::HashSet::new();
        for km_name in self.keymap_chain() {
            let Some(km) = self.keymaps.get(&km_name) else {
                continue;
            };
            for entry in km.which_key_entries(prefix, &self.commands) {
                if existing.insert(format!("{:?}", entry.key)) {
                    entries.push(entry);
                }
            }
        }
        entries
    }

    /// Get which-key entries for the current keymap, merging overlay + parent.
    /// Applies the `which-key-sort-order` option: groups first, then sorted.
    pub fn which_key_entries_for_current_keymap(&self) -> Vec<WhichKeyEntry> {
        let mut entries = self.merged_which_key_entries(&self.which_key_prefix);
        self.sort_which_key_entries(&mut entries);
        entries
    }

    /// Get all top-level bindings for the current buffer's keymap + parent.
    /// Used by `show-buffer-keys` (`?`) to show a full keybind reference.
    pub fn buffer_keys_entries(&self) -> Vec<WhichKeyEntry> {
        let mut entries = self.merged_which_key_entries(&[]);
        self.sort_which_key_entries(&mut entries);
        entries
    }

    /// Sort which-key entries: groups first (sorted by key), then leaves
    /// sorted by the chosen field (`key`, `desc`, or `none`).
    fn sort_which_key_entries(&self, entries: &mut [WhichKeyEntry]) {
        let order = self
            .get_option("which-key-sort-order")
            .map(|(v, _)| v)
            .unwrap_or_else(|| "key".to_string());
        match order.as_str() {
            "desc" => {
                entries.sort_by(|a, b| {
                    b.is_group
                        .cmp(&a.is_group)
                        .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
                });
            }
            "none" => {} // insertion order
            _ => {
                // "key" (default): groups first, then alphabetical by key
                entries.sort_by(|a, b| {
                    b.is_group.cmp(&a.is_group).then_with(|| {
                        let ak = crate::text_utils::format_keypress(&a.key);
                        let bk = crate::text_utils::format_keypress(&b.key);
                        ak.cmp(&bk)
                    })
                });
            }
        }
    }

    /// Set the which-key prefix and reset scroll to top.
    /// Use this instead of assigning `which_key_prefix` directly.
    pub fn set_which_key_prefix(&mut self, prefix: Vec<KeyPress>) {
        self.which_key_prefix = prefix;
        self.which_key_scroll = 0;
    }

    /// Clear the which-key prefix and reset scroll.
    pub fn clear_which_key_prefix(&mut self) {
        self.which_key_prefix.clear();
        self.which_key_scroll = 0;
    }

    /// Activate or deactivate the transient leader keypad. Use this instead
    /// of assigning `leader_active` directly — it keeps `leader_activated_at`
    /// and `which_key_popup_redraw_done` (the `which_key_idle_delay` timing
    /// bookkeeping, ROADMAP #83, see `editor::idle_ops`) from drifting out of
    /// sync with `leader_active` itself.
    pub fn set_leader_active(&mut self, active: bool) {
        self.leader_active = active;
        self.leader_activated_at = if active {
            Some(std::time::Instant::now())
        } else {
            None
        };
        self.which_key_popup_redraw_done = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Language;

    #[test]
    fn org_buffer_keymap_names() {
        let mut editor = Editor::new();
        editor.syntax.set_language(0, Language::Org);
        let names = editor.current_keymap_names();
        // Org keymap moved to modules/org/ — falls back to normal at construction
        assert_eq!(names, Some(("org", Some("normal"))));
    }

    // org_keymap_spc_m_s_n_widens — moved to module tests (modules/org/)
    // md_keymap_spc_m_s_n_widens — moved to module tests (modules/markdown/)

    #[test]
    fn kernel_keymap_has_no_leader_bindings() {
        // Migration invariant: the SPC leader tree lives entirely in the keymap
        // flavor module (keymap-doom, embedded + loaded at startup), NOT the
        // kernel — which keeps only vi-modal primitives. If this fails, a leader
        // binding crept back into keymaps.rs; move it to keymap-doom's
        // autoloads.scm instead (single source of truth — the old duplicated
        // kernel tree had drifted out of sync, e.g. SPC C collaboration).
        let editor = Editor::new();
        let spc = parse_key_seq("SPC");
        for (map_name, km) in &editor.keymaps {
            let leader: Vec<String> = km
                .bindings()
                .filter(|(keys, _)| keys.first() == spc.first())
                .map(|(_, cmd)| cmd.clone())
                .collect();
            assert!(
                leader.is_empty(),
                "kernel keymap '{map_name}' still defines SPC leader bindings {leader:?}; \
                 these belong in the keymap flavor module, not the kernel"
            );
        }
    }

    #[test]
    fn keymap_chain_walks_full_ancestry_so_dispatch_matches_display() {
        // Regression guard for the dual-mechanism divergence: dispatch used to
        // consult only (primary, single-fallback) while describe-bindings walked
        // the parent chain N levels. A 3-deep chain would dispatch and display
        // differently. Now both consume `keymap_chain()`, so a binding in the
        // DEEPEST layer must be reachable by the same chain the UI renders.
        use crate::keymap::{Keymap, LookupResult};
        let mut editor = Editor::new();
        // Force the primary keymap to be an overlay with a 3-deep ancestry:
        //   file-tree -> mid -> normal, with the test binding only in `mid`.
        editor.buffers[0].kind = crate::buffer::BufferKind::FileTree;
        let mut mid = Keymap::with_parent("mid", "normal");
        mid.bind(vec![KeyPress::ctrl('t')], "mid-only-command");
        editor.keymaps.insert("mid".to_string(), mid);
        editor.keymaps.insert(
            "file-tree".to_string(),
            Keymap::with_parent("file-tree", "mid"),
        );

        let chain = editor.keymap_chain();
        assert_eq!(
            chain,
            vec![
                "file-tree".to_string(),
                "mid".to_string(),
                "normal".to_string()
            ],
            "chain must walk the full parent ancestry, deduped"
        );

        // Dispatch-style resolution over the chain finds the deep binding.
        let keys = vec![KeyPress::ctrl('t')];
        let resolved =
            chain
                .iter()
                .find_map(|n| match editor.keymaps.get(n).map(|k| k.lookup(&keys)) {
                    Some(LookupResult::Exact(c)) => Some(c.to_string()),
                    _ => None,
                });
        assert_eq!(
            resolved.as_deref(),
            Some("mid-only-command"),
            "a binding in the deepest chain layer must resolve"
        );
        // Display (which-key/describe-bindings) iterates the SAME chain, so it is
        // guaranteed to surface the same binding — divergence is impossible.
    }

    #[test]
    fn leader_keypad_uses_mode_local_leader_in_org_buffers() {
        // Regression for the "SPC only shows m" bug: in an org buffer the keypad
        // chain must be [org-leader, leader] (mode-local leader first, then the
        // global leader) — NOT the org buffer keymap (which used to own `SPC m`
        // and shadow `SPC → leader-dispatch`).
        let mut editor = Editor::new();
        editor
            .syntax
            .set_language(0, crate::syntax::languages::Language::Org);
        editor.leader_active = true;
        assert_eq!(
            editor.keymap_chain(),
            vec!["org-leader".to_string(), "leader".to_string()],
            "org keypad must consult org-leader then the global leader"
        );

        // A non-org buffer with no local leader keeps the plain global leader.
        editor
            .syntax
            .set_language(0, crate::syntax::languages::Language::Rust);
        assert_eq!(editor.keymap_chain(), vec!["leader".to_string()]);
    }

    #[test]
    fn ctrl_g_resolves_to_file_info() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").unwrap();
        let keys = vec![KeyPress::ctrl('g')];
        assert_eq!(
            normal.lookup(&keys),
            crate::keymap::LookupResult::Exact("file-info")
        );
    }

    #[test]
    fn escape_resolves_to_enter_normal_mode_in_normal_mode() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").unwrap();
        let keys = vec![KeyPress::special(Key::Escape)];
        assert_eq!(
            normal.lookup(&keys),
            crate::keymap::LookupResult::Exact("enter-normal-mode")
        );
    }

    #[test]
    fn escape_in_normal_mode_dismisses_an_active_kb_preview_popup() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(mae_kb::Node::new(
            "concept:buffer",
            "Buffer",
            mae_kb::NodeKind::Concept,
            "body",
        ));
        // `kb_preview_show` requires the active buffer to be a KB-view
        // buffer (`BufferKind::Kb`) — `open_help_at` gets us there.
        editor.open_help_at("concept:buffer");
        editor.kb_preview_show("concept:buffer");
        assert!(editor.kb_preview_popup().is_some());
        // Escape resolves to "enter-normal-mode" (see the lookup test
        // above) — dispatching it is exactly what pressing Escape now
        // does, and `dispatch_builtin_inner`'s existing auto-dismiss logic
        // clears the popup as a side effect since the command isn't on
        // its exclusion list.
        editor.dispatch_builtin("enter-normal-mode");
        assert!(editor.kb_preview_popup().is_none());
    }

    #[test]
    fn escape_in_normal_mode_dismisses_an_active_hover_popup() {
        let mut editor = Editor::new();
        editor.apply_hover_result("fn main()".into());
        assert!(editor.lsp.hover_popup.is_some());
        editor.dispatch_builtin("enter-normal-mode");
        assert!(editor.lsp.hover_popup.is_none());
    }

    #[test]
    fn escape_in_normal_mode_is_a_no_op_when_no_popup_is_active() {
        let mut editor = Editor::new();
        assert_eq!(editor.mode, Mode::Normal);
        assert!(editor.kb_preview_popup().is_none());
        assert!(editor.lsp.hover_popup.is_none());
        // Must not panic, error, or change mode — Normal-mode Escape stays
        // inert when there's nothing to dismiss, same as before this fix.
        editor.dispatch_builtin("enter-normal-mode");
        assert_eq!(editor.mode, Mode::Normal);
    }

    // org_keymap_fallback_to_normal — org keymap moved to modules/org/

    // File-tree keymap + bindings moved to modules/file-tree/autoloads.scm.
    // Verify commands remain registered as kernel builtins.
    #[test]
    fn file_tree_commands_registered() {
        let editor = Editor::new();
        assert!(editor.commands.contains("file-tree-toggle"));
        assert!(editor.commands.contains("file-tree-down"));
        assert!(editor.commands.contains("file-tree-open"));
        assert!(editor.commands.contains("file-tree-create"));
    }

    #[test]
    fn file_tree_buffer_keymap_names() {
        let mut editor = Editor::new();
        let root = std::env::current_dir().unwrap();
        let tree_buf = crate::buffer::Buffer::new_file_tree(&root);
        editor.buffers.push(tree_buf);
        let tree_idx = editor.buffers.len() - 1;
        editor.window_mgr.focused_window_mut().buffer_idx = tree_idx;
        let names = editor.current_keymap_names();
        assert_eq!(names, Some(("file-tree", Some("normal"))));
    }

    #[test]
    fn help_keymap_exists_with_bindings() {
        let editor = Editor::new();
        let help_map = editor.keymaps.get("help").unwrap();
        // help now parents onto the shared `navigation` context (which itself
        // parents onto `normal`), so nav buffers get flavor-independent movement
        // + leader access. Chain: help → navigation → normal.
        assert_eq!(help_map.parent.as_deref(), Some("navigation"));
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
        let mut editor = Editor::new();
        // Create a KB buffer and focus it
        let mut buf = crate::buffer::Buffer::new();
        buf.kind = crate::buffer::BufferKind::Kb;
        buf.name = "*Help*".to_string();
        editor.buffers.push(buf);
        let help_idx = editor.buffers.len() - 1;
        editor.window_mgr.focused_window_mut().buffer_idx = help_idx;
        let names = editor.current_keymap_names();
        assert_eq!(names, Some(("help", Some("normal"))));
    }

    // dailies_bindings_registered + spc_sub_prefixes_have_which_key_group_names
    // moved to the binary-level module-load test (crates/mae bootstrap tests):
    // the SPC n d dailies bindings and which-key group names now come from the
    // keymap-doom module, which mae-core cannot load (no SchemeRuntime here).

    #[test]
    fn overlay_keymaps_have_parent_field() {
        let editor = Editor::new();
        // git-status, org, markdown keymaps moved to modules
        // Only kernel keymaps remain at construction
        assert!(editor.keymaps.get("normal").unwrap().parent.is_none());
        // Nav overlays now parent onto the shared `navigation` context.
        assert_eq!(
            editor.keymaps.get("help").unwrap().parent.as_deref(),
            Some("navigation")
        );
        assert_eq!(
            editor.keymaps.get("navigation").unwrap().parent.as_deref(),
            Some("normal")
        );
    }

    // git_status_q_binds_to_kill_buffer — git-status keymap moved to modules/git-status/
    // overlay_keymaps_have_show_buffer_keys — git-status, debug keymaps moved to modules/
    // git_status_spc_m_local_leader — git-status keymap moved to modules/git-status/

    #[test]
    fn overlay_keymaps_have_show_buffer_keys_help() {
        let editor = Editor::new();
        let q_key = parse_key_seq("?");
        // Only help keymap remains in kernel
        let km = editor.keymaps.get("help").unwrap();
        assert_eq!(
            km.lookup(&q_key),
            crate::keymap::LookupResult::Exact("show-buffer-keys"),
        );
    }

    #[test]
    fn buffer_keys_entries_returns_entries() {
        let mut editor = Editor::new();
        // Create a KB buffer and focus it (help keymap is still in kernel)
        let mut buf = crate::buffer::Buffer::new();
        buf.kind = crate::buffer::BufferKind::Kb;
        buf.name = "*Help*".to_string();
        editor.buffers.push(buf);
        let idx = editor.buffers.len() - 1;
        editor.window_mgr.focused_window_mut().buffer_idx = idx;
        let entries = editor.buffer_keys_entries();
        // Should have entries from help + normal keymaps
        assert!(!entries.is_empty());
    }

    #[test]
    fn shift_i_bound_in_normal_mode() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").unwrap();
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
        let mut editor = Editor::new();
        editor.syntax.set_language(0, Language::Org);
        let (primary, fallback) = editor.current_keymap_names().unwrap();
        assert_eq!(primary, "org");
        assert_eq!(fallback, Some("normal"));
    }
}
