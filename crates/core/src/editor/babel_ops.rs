//! Babel and export operations on the Editor.

use std::path::PathBuf;

use super::Editor;
use crate::babel::{self, execute::BabelExecutor, results, tangle};
use crate::export::{self, html::HtmlExporter, markdown::MarkdownExporter, Exporter};

impl Editor {
    /// Build a babel executor seeded from the editor's babel options: the
    /// persistent sessions, timeout, and the configurable compiler binaries /
    /// C++ standard for compiled blocks (principle #8 — no hardcoded `c++`/`cc`).
    fn new_babel_executor(&mut self) -> BabelExecutor {
        let mut executor = BabelExecutor {
            sessions: std::mem::take(&mut self.babel_sessions),
            timeout_secs: self.babel_timeout,
            ..BabelExecutor::default()
        };
        executor.compiled.cxx = self.babel_cxx_compiler.clone();
        executor.compiled.cc = self.babel_c_compiler.clone();
        executor.compiled.cxx_std = self.babel_cxx_std.clone();
        executor.shell_env_enabled = self.babel_inherit_shell_env;
        // `sessions` is REUSED (persistent across calls, per :session
        // semantics), not freshly constructed — its shell_env_enabled must
        // be re-applied here too, not just at SessionManager::default()
        // time, so a live `:set babel_inherit_shell_env` takes effect for
        // sessions created afterward even on an already-existing manager.
        executor.sessions.shell_env_enabled = self.babel_inherit_shell_env;
        executor
    }

    /// Execute the source block at the cursor position.
    /// Uses AI-aware buffer/cursor targeting so the AI agent can execute
    /// blocks in a non-focused buffer via `set_ai_target`.
    ///
    /// `interactive` distinguishes the human keybinding path from the
    /// AI/MCP tool-call path (#269) — it is NOT inferred from
    /// `self.ai.target_buffer_idx`, which is a buffer-*targeting*
    /// mechanism, not an invocation-source flag (a human could trigger this
    /// interactively while an AI target happens to be set from a prior
    /// session). This matters specifically for `NeedsConfirmation`: the
    /// interactive path can open a confirm dialog and wait; the AI/MCP path
    /// has no human to answer one, so it must refuse outright instead.
    pub fn babel_execute(&mut self, interactive: bool) -> Result<String, String> {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();
        let cursor_line = self.ai_cursor_row();

        let blocks = babel::parse_src_blocks(&source);
        let block = match babel::find_block_at_line(&blocks, cursor_line) {
            Some(b) => b.clone(),
            None => {
                let msg = "No source block at cursor".to_string();
                self.set_status(msg.clone());
                return Err(msg);
            }
        };

        // Check eval policy
        let file_path = self.buffers[buf_idx].file_path().map(PathBuf::from);
        let policy = babel::safety::effective_eval_policy(
            &block.header_args.eval,
            file_path.as_deref(),
            &self.babel_trust_paths,
            self.babel_confirm,
        );

        match policy {
            babel::safety::EffectivePolicy::Blocked => {
                let msg = "Block execution blocked by :eval never".to_string();
                self.set_status(msg.clone());
                Err(msg)
            }
            babel::safety::EffectivePolicy::NeedsConfirmation => {
                if interactive {
                    self.mini_dialog = Some(crate::command_palette::MiniDialogState::confirm(
                        format!(
                            "Execute {} block? (:eval requires confirmation)",
                            block.language
                        ),
                        crate::command_palette::MiniDialogContext::BabelConfirm {
                            buf_idx,
                            block: Box::new(block),
                        },
                    ));
                    Ok("Waiting for confirmation…".to_string())
                } else {
                    Err("Block requires human confirmation (:eval query/confirm) — \
                         not available via AI/MCP call"
                        .to_string())
                }
            }
            babel::safety::EffectivePolicy::Allow => Ok(self.babel_run_block(buf_idx, &block)),
        }
    }

    /// Run a babel block for real (post-policy-check execution: resolve
    /// vars → execute → format results → edit the buffer → status).
    /// Shared by `babel_execute`'s immediate `Allow` path and the
    /// `MiniDialogContext::BabelConfirm` apply arm once the user confirms
    /// (#269) — avoids duplicating this logic between the two trigger
    /// points. Re-reads the buffer's *current* content rather than trusting
    /// anything captured when a confirm dialog was opened, since edits may
    /// have happened in between (the same staleness tradeoff every other
    /// MiniDialog confirm-then-effect context already accepts).
    pub fn babel_run_block(&mut self, buf_idx: usize, block: &babel::SrcBlock) -> String {
        let source = self.buffers[buf_idx].rope().to_string();
        let blocks = babel::parse_src_blocks(&source);
        let file_path = self.buffers[buf_idx].file_path().map(PathBuf::from);
        let resolved_vars = babel::vars::resolve_vars(block, &blocks, &source);

        let buf_dir = file_path
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or_else(|| std::path::Path::new("."));

        let mut executor = self.new_babel_executor();
        let result = executor.execute_block(block, buf_dir, &resolved_vars);
        self.babel_sessions = executor.sessions;

        let output_text = match &result {
            babel::execute::ExecResult::Output(s) => s.clone(),
            babel::execute::ExecResult::Value(s) => s.clone(),
            babel::execute::ExecResult::File(p) => format!("[[file:{}]]", p.display()),
            babel::execute::ExecResult::PendingSchemeEval(code) => {
                self.pending_scheme_eval.push(code.clone());
                let msg = "Scheme block queued for evaluation".to_string();
                self.set_status(msg.clone());
                return msg;
            }
            babel::execute::ExecResult::PendingDatalogQuery(query) => {
                self.dispatch_kb_raw_query(query);
                return self.status_msg.clone();
            }
            babel::execute::ExecResult::Error(e) => {
                let msg = format!("Babel error: {}", e);
                self.set_status(msg.clone());
                return msg;
            }
        };

        let formatted = results::format_results(&output_text, &block.header_args.results);
        let (del_start, del_end, insert_text) = results::compute_results_edit(
            &source,
            block.line_range.1,
            block.name.as_deref(),
            &formatted,
        );

        // Apply edit atomically
        let buf = &mut self.buffers[buf_idx];
        buf.begin_undo_group();
        if del_start < del_end {
            buf.delete_range(del_start, del_end);
        }
        buf.insert_text_at(del_start, &insert_text);
        buf.end_undo_group();

        // Post-insertion fixups
        self.clamp_all_cursors();
        self.mark_full_redraw();
        let msg = format!("Executed {} block", block.language);
        self.set_status(msg.clone());
        msg
    }

    /// Execute all source blocks in the current buffer.
    /// Uses AI-aware buffer targeting.
    ///
    /// @ai-caution: [security] Every block MUST clear
    /// `babel::safety::effective_eval_policy`, exactly as the single-block
    /// `babel_execute` does. Testing only `header_args.eval == Never` (as this
    /// did before audit #596.1) silently ignored the two gates that actually
    /// protect a *file you did not write*: `:eval query` blocks, and `:eval yes`
    /// blocks in a file outside `babel_trust_paths` while `babel_confirm` is on.
    /// "Execute all" is precisely the operation a hostile org file wants you to
    /// run, so it must be the *stricter* path, never the looser one. Blocks that
    /// need a human answer are skipped and counted rather than executed — one
    /// command cannot open N sequential confirm dialogs.
    pub fn babel_execute_all(&mut self) {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();
        let blocks = babel::parse_src_blocks(&source);

        if blocks.is_empty() {
            self.set_status("No source blocks in buffer");
            return;
        }

        let count = blocks.len();
        let mut executed = 0usize;
        let mut blocked = 0usize;
        let mut needs_confirm = 0usize;
        // Execute blocks in reverse order to preserve line offsets
        for i in (0..count).rev() {
            // Re-read source after each edit
            let current_source = self.buffers[buf_idx].rope().to_string();
            let current_blocks = babel::parse_src_blocks(&current_source);
            if i >= current_blocks.len() {
                continue;
            }
            let block = &current_blocks[i];

            let file_path = self.buffers[buf_idx].file_path().map(PathBuf::from);
            match babel::safety::effective_eval_policy(
                &block.header_args.eval,
                file_path.as_deref(),
                &self.babel_trust_paths,
                self.babel_confirm,
            ) {
                babel::safety::EffectivePolicy::Allow => {}
                babel::safety::EffectivePolicy::Blocked => {
                    blocked += 1;
                    continue;
                }
                babel::safety::EffectivePolicy::NeedsConfirmation => {
                    needs_confirm += 1;
                    continue;
                }
            }

            let buf_dir = file_path
                .as_ref()
                .and_then(|p| p.parent())
                .unwrap_or_else(|| std::path::Path::new("."));

            let resolved_vars = babel::vars::resolve_vars(block, &current_blocks, &current_source);
            let mut executor = self.new_babel_executor();

            let result = executor.execute_block(block, buf_dir, &resolved_vars);
            self.babel_sessions = executor.sessions;
            let output_text = match &result {
                babel::execute::ExecResult::Output(s) => s.clone(),
                babel::execute::ExecResult::Value(s) => s.clone(),
                babel::execute::ExecResult::File(p) => format!("[[file:{}]]", p.display()),
                babel::execute::ExecResult::PendingSchemeEval(_)
                | babel::execute::ExecResult::PendingDatalogQuery(_)
                | babel::execute::ExecResult::Error(_) => continue,
            };

            let formatted = results::format_results(&output_text, &block.header_args.results);
            let (del_start, del_end, insert_text) = results::compute_results_edit(
                &current_source,
                block.line_range.1,
                block.name.as_deref(),
                &formatted,
            );

            let buf = &mut self.buffers[buf_idx];
            buf.begin_undo_group();
            if del_start < del_end {
                buf.delete_range(del_start, del_end);
            }
            buf.insert_text_at(del_start, &insert_text);
            buf.end_undo_group();
            executed += 1;
        }

        self.clamp_all_cursors();
        self.mark_full_redraw();
        // Report what actually ran, not the block count (ADR-086: a skipped
        // block must not be indistinguishable from an executed one).
        let mut msg = format!("Executed {executed} of {count} block(s)");
        if blocked > 0 {
            msg.push_str(&format!("; {blocked} blocked by :eval never"));
        }
        if needs_confirm > 0 {
            msg.push_str(&format!(
                "; {needs_confirm} need confirmation (run babel-execute on each, \
                 or add this file to babel-trust-paths)"
            ));
        }
        self.set_status(msg);
    }

    /// Tangle all source blocks in the current buffer.
    /// Uses AI-aware buffer targeting.
    pub fn babel_tangle(&mut self) -> String {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();

        let file_path = self.buffers[buf_idx].file_path().map(PathBuf::from);
        let base_dir = file_path
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or_else(|| std::path::Path::new("."));
        let base_name = file_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("untitled");

        let outputs = tangle::tangle_buffer(&source, base_dir, base_name);
        if outputs.is_empty() {
            let msg = "No blocks with :tangle directive".to_string();
            self.set_status(msg.clone());
            return msg;
        }

        let results = tangle::write_tangle_outputs(&outputs, true);
        let mut success_count = 0;
        let mut errors = Vec::new();
        for result in results {
            match result {
                Ok(_) => success_count += 1,
                Err(e) => errors.push(e),
            }
        }

        let msg = if errors.is_empty() {
            format!("Tangled {} file(s)", success_count)
        } else {
            format!(
                "Tangled {} file(s), {} error(s): {}",
                success_count,
                errors.len(),
                errors[0]
            )
        };
        self.set_status(msg.clone());
        msg
    }

    /// Kill all babel session processes.
    pub fn babel_kill_sessions(&mut self) {
        let count = self.babel_sessions.len();
        self.babel_sessions.kill_all();
        if count > 0 {
            self.set_status(format!("Killed {} babel session(s)", count));
        } else {
            self.set_status("No active babel sessions");
        }
    }

    /// Open a source block in a dedicated edit buffer with proper language mode.
    /// Emacs equivalent: `C-c '` / `org-edit-special`. MAE uses `SPC m '`.
    pub fn babel_edit_special(&mut self) {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();
        let cursor_line = self.ai_cursor_row();

        let blocks = babel::parse_src_blocks(&source);
        let block = match babel::find_block_at_line(&blocks, cursor_line) {
            Some(b) => b.clone(),
            None => {
                self.set_status("No source block at cursor");
                return;
            }
        };

        let buf_name = format!(
            "*babel-edit: {} [{}]*",
            block.name.as_deref().unwrap_or("src"),
            block.language
        );

        // Check if edit buffer already exists
        if self.buffers.iter().any(|b| b.name == buf_name) {
            self.set_status("Edit buffer already open for this block");
            return;
        }

        let ctx = crate::buffer::BabelEditContext {
            source_buffer: buf_idx,
            block_line_range: block.line_range,
            body_char_range: block.body_char_range,
            block_name: block.name.clone(),
            language: block.language.clone(),
        };

        let mut edit_buf = crate::Buffer::new();
        edit_buf.name = buf_name.clone();
        edit_buf.insert_text_at(0, &block.body);
        edit_buf.modified = false;
        edit_buf.babel_edit_source = Some(ctx);

        self.buffers.push(edit_buf);
        let new_idx = self.buffers.len() - 1;

        // Switch to new buffer in the focused window
        let win = self.window_mgr.focused_window_mut();
        win.buffer_idx = new_idx;
        win.cursor_row = 0;
        win.cursor_col = 0;
        win.scroll_offset = 0;

        self.mark_full_redraw();
        self.set_status(format!(
            "Editing {} block — SPC m ' to commit",
            block.language
        ));
    }

    /// Commit changes from an edit-special buffer back to the source buffer.
    /// Emacs equivalent: `C-c '` in the edit buffer. MAE uses `SPC m '`.
    pub fn babel_edit_commit(&mut self) {
        let buf_idx = self.ai_active_buffer_idx();

        let ctx = match self.buffers[buf_idx].babel_edit_source.take() {
            Some(ctx) => ctx,
            None => {
                self.set_status("Not in a babel edit buffer");
                return;
            }
        };

        // Read the edited content
        let new_body = self.buffers[buf_idx].rope().to_string();

        // Validate source buffer still exists
        if ctx.source_buffer >= self.buffers.len() {
            self.set_status("Source buffer no longer exists");
            return;
        }

        // Replace body in source buffer
        let src_buf = &mut self.buffers[ctx.source_buffer];
        src_buf.begin_undo_group();
        if ctx.body_char_range.0 < ctx.body_char_range.1 {
            src_buf.delete_range(ctx.body_char_range.0, ctx.body_char_range.1);
        }
        src_buf.insert_text_at(ctx.body_char_range.0, &new_body);
        src_buf.end_undo_group();

        // Kill edit buffer and switch back to source
        let source_idx = ctx.source_buffer;
        self.kill_buffer_at(buf_idx);

        // Adjust source_idx if needed (kill might shift indices)
        let target_idx = if buf_idx < source_idx {
            source_idx - 1
        } else {
            source_idx
        };

        let win = self.window_mgr.focused_window_mut();
        if target_idx < self.buffers.len() {
            win.buffer_idx = target_idx;
        }

        self.clamp_all_cursors();
        self.mark_full_redraw();
        self.set_status("Committed edit back to source buffer");
    }

    /// Export current org buffer to HTML.
    pub fn org_export_html(&mut self) -> String {
        self.org_export_to("html")
    }

    /// Export current org buffer to Markdown.
    pub fn org_export_markdown(&mut self) -> String {
        self.org_export_to("markdown")
    }

    /// Export subtree at cursor.
    /// Uses AI-aware buffer/cursor targeting.
    pub fn org_export_subtree(&mut self) {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();
        let cursor_line = self.ai_cursor_row();

        let (mut meta, elements, element_lines) = export::parse_org_document_with_lines(&source);
        meta.options.allow_raw_html_export_blocks = self.org_export_allow_raw_html_blocks;

        // The last heading starting at or before the cursor's line.
        //
        // Audit #596.2: this read `elements.iter().enumerate().enumerate()`,
        // which yields the SAME index twice — so `current_line` was an element
        // ordinal, not a line. Any cursor past the element count therefore fell
        // through the whole document and exported its last subtree instead of
        // the one under the cursor.
        let heading_idx = elements
            .iter()
            .enumerate()
            .take_while(|(i, _)| element_lines[*i] <= cursor_line)
            .filter(|(_, el)| matches!(el, export::OrgElement::Heading { .. }))
            .map(|(i, _)| i)
            .last();

        let subtree = match heading_idx {
            Some(idx) => export::extract_subtree(&elements, idx),
            None => {
                self.set_status("No heading at cursor for subtree export");
                return;
            }
        };

        let exporter = HtmlExporter;
        let output = exporter.export(&meta, &subtree);

        // Write to file
        let file_path = self.buffers[buf_idx].file_path().map(PathBuf::from);
        let output_path = file_path
            .as_ref()
            .map(|p| p.with_extension("subtree.html"))
            .unwrap_or_else(|| PathBuf::from("export-subtree.html"));

        match std::fs::write(&output_path, &output) {
            Ok(()) => {
                self.set_status(format!("Exported subtree to {}", output_path.display()));
            }
            Err(e) => {
                self.set_status(format!("Export failed: {}", e));
            }
        }
    }

    /// Internal: export to a given format.
    /// Uses AI-aware buffer targeting.
    fn org_export_to(&mut self, format: &str) -> String {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();
        let (mut meta, elements) = export::parse_org_document(&source);
        meta.options.allow_raw_html_export_blocks = self.org_export_allow_raw_html_blocks;

        // Apply tag filtering
        let filtered = if !meta.select_tags.is_empty() || !meta.exclude_tags.is_empty() {
            export::filter_by_tags(&elements, &meta.select_tags, &meta.exclude_tags)
        } else {
            elements
        };

        let (output, ext) = match format {
            "html" => {
                let exporter = HtmlExporter;
                (exporter.export(&meta, &filtered), "html")
            }
            "markdown" | "md" => {
                let exporter = MarkdownExporter;
                (exporter.export(&meta, &filtered), "md")
            }
            _ => {
                let msg = format!("Unknown export format: {}", format);
                self.set_status(msg.clone());
                return msg;
            }
        };

        // Write to file alongside the org file
        let file_path = self.buffers[buf_idx].file_path().map(PathBuf::from);
        let output_path = file_path
            .as_ref()
            .map(|p| p.with_extension(ext))
            .unwrap_or_else(|| PathBuf::from(format!("export.{}", ext)));

        let msg = match std::fs::write(&output_path, &output) {
            Ok(()) => format!("Exported to {}", output_path.display()),
            Err(e) => format!("Export failed: {}", e),
        };
        self.set_status(msg.clone());
        msg
    }

    /// Convert current Markdown buffer to Org format (in-buffer).
    /// Uses AI-aware buffer targeting.
    pub fn markdown_to_org(&mut self) {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();
        let (meta, elements) = export::markdown_parser::parse_markdown_document(&source);
        let writer = export::org_writer::OrgWriter;
        let org_text = export::Exporter::export(&writer, &meta, &elements);
        self.buffers[buf_idx].replace_contents(&org_text);
        self.set_status("Converted Markdown → Org");
    }

    /// Convert current Org buffer to Markdown (in-buffer).
    /// Uses AI-aware buffer targeting.
    pub fn org_to_markdown_buffer(&mut self) {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();
        let (meta, elements) = export::parse_org_document(&source);
        let exporter = MarkdownExporter;
        let md_text = export::Exporter::export(&exporter, &meta, &elements);
        self.buffers[buf_idx].replace_contents(&md_text);
        self.set_status("Converted Org → Markdown");
    }

    /// List KB instances — returns structured info for AI tools.
    pub fn kb_instances(&mut self) -> String {
        if self.kb.registry.instances.is_empty() {
            let msg = "KB federation: built-in KB only (no external instances registered)";
            self.set_status(msg);
            return msg.to_string();
        }

        let mut lines = vec![format!(
            "KB federation: {} instance(s)",
            self.kb.registry.instances.len()
        )];
        for inst in &self.kb.registry.instances {
            let count = self
                .kb
                .instances
                .get(&inst.uuid)
                .map(|kb| kb.len())
                .unwrap_or(0);
            lines.push(format!(
                "  {} [{}]: {} nodes, enabled={}, dir={}",
                inst.name,
                &inst.uuid[..8.min(inst.uuid.len())],
                count,
                inst.enabled,
                inst.org_dir.display(),
            ));
        }
        let summary = lines.join("\n");
        self.set_status(&lines[0]);
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
