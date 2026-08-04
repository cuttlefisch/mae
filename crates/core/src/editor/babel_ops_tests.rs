//! Tests for [`super`] — org-babel execution ops.
//!
//! Extracted under CLAUDE.md's file-ceiling remedy (987 lines, ~374 of them
//! inline tests). `#[path]` adds a module level, so the inner `mod tests`
//! uses `use super::super::*`.

#[cfg(test)]
mod tests {
    use super::super::*;

    fn editor_with_block(src: &str) -> Editor {
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, src);
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor
    }

    // "scheme" blocks resolve to `ExecResult::PendingSchemeEval` (no process
    // spawn, no compiler) — the fastest, most hermetic way to exercise the
    // confirm-gate's *decision* logic without depending on an execution
    // backend. `#269`.
    const NEEDS_CONFIRM_BLOCK: &str =
        "#+begin_src scheme :eval query\n(display \"hi\")\n#+end_src\n";
    const BLOCKED_BLOCK: &str = "#+begin_src scheme :eval never\n(display \"hi\")\n#+end_src\n";
    const ALLOWED_BLOCK: &str = "#+begin_src scheme\n(display \"hi\")\n#+end_src\n";

    #[test]
    fn babel_execute_interactive_needs_confirmation_opens_dialog_without_executing() {
        let mut editor = editor_with_block(NEEDS_CONFIRM_BLOCK);
        let result = editor.babel_execute(true);
        assert!(
            result.is_ok(),
            "should return Ok (pending), not Err: {:?}",
            result
        );
        assert!(
            editor.pending_scheme_eval.is_empty(),
            "the block must NOT execute while awaiting confirmation"
        );
        match &editor.mini_dialog {
            Some(dialog) => match &dialog.context {
                crate::command_palette::MiniDialogContext::BabelConfirm { .. } => {}
                other => panic!("expected BabelConfirm context, got {:?}", other),
            },
            None => panic!("expected a mini_dialog to be opened"),
        }
    }

    #[test]
    fn babel_execute_ai_needs_confirmation_refuses() {
        let mut editor = editor_with_block(NEEDS_CONFIRM_BLOCK);
        let result = editor.babel_execute(false);
        assert!(
            result.is_err(),
            "AI/MCP path must refuse, not silently allow"
        );
        assert!(
            editor.pending_scheme_eval.is_empty(),
            "a refused block must not execute"
        );
        assert!(
            editor.mini_dialog.is_none(),
            "the AI path has no human to answer a dialog — none should open"
        );
    }

    #[test]
    fn babel_execute_blocked_refuses_both_paths() {
        for interactive in [true, false] {
            let mut editor = editor_with_block(BLOCKED_BLOCK);
            let result = editor.babel_execute(interactive);
            assert!(
                result.is_err(),
                ":eval never must refuse regardless of interactive={}",
                interactive
            );
            assert!(editor.pending_scheme_eval.is_empty());
            assert!(
                editor.mini_dialog.is_none(),
                "a hard block never needs a confirm dialog"
            );
        }
    }

    #[test]
    fn babel_execute_allow_executes_immediately_both_paths() {
        for interactive in [true, false] {
            let mut editor = editor_with_block(ALLOWED_BLOCK);
            // `babel_confirm` (global) defaults to true, which would push
            // even a default-policy block to NeedsConfirmation for an
            // untrusted/pathless test buffer — set it false to construct a
            // genuine Allow case, matching a user who has disabled the
            // global confirm gate.
            editor.babel_confirm = false;
            let result = editor.babel_execute(interactive);
            assert!(
                result.is_ok(),
                "an allowed block must execute: {:?}",
                result
            );
            assert_eq!(
                editor.pending_scheme_eval.len(),
                1,
                "an allowed block executes immediately, unchanged from before #269"
            );
        }
    }

    #[test]
    fn babel_confirm_apply_executes_the_deferred_block() {
        // Mirrors the resume path `apply_mini_dialog` drives on confirm —
        // exercised here at the `babel_run_block` level (the shared
        // execution helper both the Allow path and the confirm-dialog path
        // call), since `apply_mini_dialog` itself lives in the `mae` binary
        // crate and is covered by its own test alongside `FileDelete`'s.
        let mut editor = editor_with_block(NEEDS_CONFIRM_BLOCK);
        editor.babel_execute(true).unwrap();
        let block = match editor.mini_dialog.take().unwrap().context {
            crate::command_palette::MiniDialogContext::BabelConfirm { block, .. } => block,
            other => panic!("expected BabelConfirm, got {:?}", other),
        };
        assert!(editor.pending_scheme_eval.is_empty(), "not yet executed");
        editor.babel_run_block(0, &block);
        assert_eq!(
            editor.pending_scheme_eval.len(),
            1,
            "confirming must actually run the deferred block"
        );
    }

    #[test]
    fn babel_run_block_results_land_after_end_src_with_multibyte_content_earlier() {
        // End-to-end regression guard (through a real `Buffer`, not just the
        // string-level compute_results_edit unit tests) for the reported bug:
        // output landing mid-word in a heading that follows the block,
        // caused by a byte/char offset mismatch anywhere multi-byte content
        // (em dash, checkmark, accented letters) preceded the block.
        let src = "* Café \u{2014} Notes\nSome text: \u{2192} \u{2713}\n\n\
                   #+begin_src sh\necho hi\n#+end_src\n\n** Downstream Section\n";
        let mut editor = editor_with_block(src);
        let blocks = babel::parse_src_blocks(&editor.buffers[0].rope().to_string());
        editor.babel_run_block(0, &blocks[0]);

        let result = editor.buffers[0].rope().to_string();
        assert!(
            result.contains("#+end_src\n\n#+RESULTS:\n: hi\n\n** Downstream Section"),
            "results must land directly after #+end_src and the following heading \
             must survive intact — got:\n{result}"
        );
    }

    // --- babel-execute-all confirm gate (audit #596.1 / #596.5) ---

    /// Three blocks whose echoed markers say *which* one ran, so the oracle
    /// pins the specific block rather than a count that could come out right
    /// for the wrong reason. The marker is *computed* by the shell
    /// (`$((1 + 1))` -> `2`) so the asserted string can only appear in the
    /// buffer if the block actually EXECUTED — searching for a literal that is
    /// already present in the block's own source would pass unconditionally.
    const HOSTILE_FILE: &str = "\
#+begin_src sh :eval query\necho QUERY-$((1 + 1))\n#+end_src\n\n\
#+begin_src sh\necho DEFAULT-$((1 + 1))\n#+end_src\n\n\
#+begin_src sh :eval never\necho NEVER-$((1 + 1))\n#+end_src\n";

    /// The strings that exist ONLY in executed output, never in the source.
    const QUERY_OUT: &str = "QUERY-2";
    const DEFAULT_OUT: &str = "DEFAULT-2";
    const NEVER_OUT: &str = "NEVER-2";

    /// A per-test directory, created for real: babel runs a block with the
    /// file's parent as cwd, so a made-up path would fail the *spawn* and mask
    /// whether the confirm gate or the shell refused. Named per test (never a
    /// shared path) so these stay parallel-safe.
    fn editor_in_temp_dir(src: &str, test_name: &str) -> (Editor, PathBuf) {
        let dir = std::env::temp_dir().join(format!("mae-babel-gate-{test_name}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("notes.org");
        let mut editor = editor_with_block(src);
        editor.buffers[0].set_file_path(file);
        (editor, dir)
    }

    /// Audit #596.1 — the attacker's test. "Execute all" is exactly the command
    /// a hostile org file wants you to run, so it must be at least as strict as
    /// single-block execute. Before the fix it consulted only
    /// `header_args.eval == Never`, so a `:eval query` block — which
    /// `babel_execute` refuses without a human answer — ran unprompted, as did
    /// any default block in a file outside `babel_trust_paths`.
    #[test]
    fn execute_all_refuses_blocks_the_confirm_gate_would_stop() {
        let (mut editor, _dir) = editor_in_temp_dir(HOSTILE_FILE, "refuses");
        assert!(
            editor.babel_confirm,
            "precondition: the global confirm gate is on by default"
        );
        assert!(
            editor.babel_trust_paths.is_empty(),
            "precondition: this file is not pre-trusted"
        );

        editor.babel_execute_all();

        let text = editor.buffers[0].rope().to_string();
        // Selective oracle: no marker from ANY block reached the buffer.
        for marker in [QUERY_OUT, DEFAULT_OUT, NEVER_OUT] {
            assert!(
                !text.contains(marker),
                "{marker} executed despite the confirm gate — got:\n{text}"
            );
        }
        assert!(
            !text.contains("#+RESULTS:"),
            "no results block should have been written at all — got:\n{text}"
        );
        // ADR-086: the status must not claim work that did not happen.
        let status = editor.status_msg.clone();
        assert!(
            status.contains("Executed 0 of 3"),
            "status must report what actually ran, got: {status:?}"
        );
        assert!(
            status.contains("need confirmation") && status.contains("blocked"),
            "status must distinguish the two skip reasons, got: {status:?}"
        );
    }

    /// The complementary case: with the gate satisfied, the blocks it allows DO
    /// run and the one it hard-blocks still does not. A refusal test alone
    /// would pass on a function that refuses everything.
    #[test]
    fn execute_all_runs_exactly_the_blocks_the_gate_allows() {
        let (mut editor, _dir) = editor_in_temp_dir(HOSTILE_FILE, "allows");
        editor.babel_confirm = false; // user disabled the global gate

        editor.babel_execute_all();

        let text = editor.buffers[0].rope().to_string();
        assert!(text.contains(DEFAULT_OUT), "got:\n{text}");
        assert!(
            !text.contains(NEVER_OUT),
            ":eval never must still refuse even with babel_confirm off — got:\n{text}"
        );
        // `:eval query` is NeedsConfirmation unconditionally — turning off the
        // *global* gate must not silently downgrade a per-block request for a
        // human answer.
        assert!(
            !text.contains(QUERY_OUT),
            ":eval query must not be downgraded by babel_confirm=false — got:\n{text}"
        );
    }

    /// Audit #596.5 — `babel_trust_paths` was a field nothing could ever write:
    /// no `options.rs` entry, no setter, so `is_trusted_path` was dead code and
    /// the whole trust axis of `effective_eval_policy` was unreachable. Drive it
    /// through the real OptionRegistry (`set_option`/`get_option`), not by
    /// poking the field, so the test fails if the registration regresses.
    #[test]
    fn babel_trust_paths_option_grants_trust_through_the_registry() {
        let (mut editor, dir) = editor_in_temp_dir(HOSTILE_FILE, "trusted");
        assert!(editor.babel_confirm, "the global gate stays ON");

        let pattern = format!("{}/*", dir.display());
        editor
            .set_option("babel_trust_paths", &format!("{pattern}, /nonexistent/*"))
            .expect("babel_trust_paths must be a registered, settable option");
        assert_eq!(
            editor.get_option("babel_trust_paths").map(|(v, _)| v),
            Some(format!("{pattern},/nonexistent/*")),
            "the value must round-trip through the registry"
        );

        editor.babel_execute_all();

        let text = editor.buffers[0].rope().to_string();
        assert!(
            text.contains(DEFAULT_OUT),
            "a trusted path must allow :eval yes without a prompt — got:\n{text}"
        );
        // Trust grants the `:eval yes` gate only. It is NOT a master key: an
        // explicit per-block `never`/`query` still wins.
        assert!(!text.contains(NEVER_OUT), "got:\n{text}");
        assert!(!text.contains(QUERY_OUT), "got:\n{text}");
    }

    /// A trust pattern that does NOT match must not grant anything — the
    /// negative half of the pair above, guarding against a matcher that
    /// accidentally returns true for a non-empty pattern list.
    #[test]
    fn babel_trust_paths_does_not_grant_a_non_matching_file() {
        let (mut editor, _dir) = editor_in_temp_dir(HOSTILE_FILE, "nonmatching");
        editor
            .set_option("babel_trust_paths", "/definitely/not/this/path/*")
            .unwrap();

        editor.babel_execute_all();

        let text = editor.buffers[0].rope().to_string();
        assert!(
            !text.contains(DEFAULT_OUT),
            "a non-matching trust pattern must not grant execution — got:\n{text}"
        );
    }

    // --- org-export-subtree cursor mapping (audit #596.2) ---

    /// Audit #596.2 — `elements.iter().enumerate().enumerate()` yields the same
    /// index twice, so the "current line" the loop compared against the cursor
    /// was really an ELEMENT ORDINAL. Any cursor line past the element count
    /// fell through the whole document and exported its LAST subtree, whatever
    /// the cursor was actually on. The bug is invisible in a document with more
    /// elements than lines, so the fixture below is deliberately line-heavy:
    /// three sections, each with padding, cursor placed in the FIRST one.
    #[test]
    fn export_subtree_picks_the_heading_at_the_cursor_not_the_last_one() {
        let src = "\
* Alpha
alpha body one
alpha body two

* Beta
beta body one
beta body two

* Gamma
gamma body one
gamma body two
";
        let dir = std::env::temp_dir().join("mae-export-subtree-cursor");
        std::fs::create_dir_all(&dir).unwrap();

        // Each cursor line and the heading whose subtree must be exported.
        // Covers the heading line itself, a body line under it, and the final
        // section — so a "first heading always" regression fails too.
        for (cursor_row, expected, forbidden) in [
            (0usize, "Alpha", "Gamma"),
            (1, "Alpha", "Gamma"),
            (2, "Alpha", "Beta"),
            (4, "Beta", "Gamma"),
            (6, "Beta", "Alpha"),
            (8, "Gamma", "Alpha"),
            (10, "Gamma", "Beta"),
        ] {
            let out = dir.join(format!("row{cursor_row}.org"));
            let mut editor = editor_with_block(src);
            editor.buffers[0].set_file_path(out.clone());
            editor.window_mgr.focused_window_mut().cursor_row = cursor_row;
            editor.org_export_subtree();

            let html = std::fs::read_to_string(out.with_extension("subtree.html"))
                .unwrap_or_else(|e| panic!("row {cursor_row}: no export written ({e})"));
            assert!(
                html.contains(expected),
                "cursor on row {cursor_row} must export the '{expected}' subtree, got:\n{html}"
            );
            assert!(
                !html.contains(forbidden),
                "cursor on row {cursor_row} must NOT export '{forbidden}', got:\n{html}"
            );
        }
    }

    #[test]
    fn babel_run_block_replaces_rather_than_stacks_results_on_second_run() {
        let src = "* Café notes\n\n#+begin_src sh\necho hi\n#+end_src\n";
        let mut editor = editor_with_block(src);

        let blocks = babel::parse_src_blocks(&editor.buffers[0].rope().to_string());
        editor.babel_run_block(0, &blocks[0]);
        let after_first = editor.buffers[0].rope().to_string();
        assert_eq!(after_first.matches("#+RESULTS:").count(), 1);

        let blocks = babel::parse_src_blocks(&after_first);
        editor.babel_run_block(0, &blocks[0]);
        let after_second = editor.buffers[0].rope().to_string();
        assert_eq!(
            after_second.matches("#+RESULTS:").count(),
            1,
            "re-running the same block must replace, not stack, the results block — got:\n{after_second}"
        );
    }
}
