//! Babel and export operations on the Editor.

use std::path::PathBuf;

use super::Editor;
use crate::babel::{self, execute::BabelExecutor, results, tangle};
use crate::export::{self, html::HtmlExporter, markdown::MarkdownExporter, Exporter};

impl Editor {
    /// Execute the source block at the cursor position.
    /// Uses AI-aware buffer/cursor targeting so the AI agent can execute
    /// blocks in a non-focused buffer via `set_ai_target`.
    pub fn babel_execute(&mut self) {
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
                self.set_status("Block execution blocked by :eval never");
                return;
            }
            babel::safety::EffectivePolicy::NeedsConfirmation => {
                // For now, show message and allow. TODO: minibuffer confirm
                self.set_status(format!(
                    "Executing {} block (confirm not yet implemented)",
                    block.language
                ));
            }
            babel::safety::EffectivePolicy::Allow => {}
        }

        // Resolve variables
        let resolved_vars = babel::vars::resolve_vars(&block, &blocks, &source);

        // Execute
        let buf_dir = file_path
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or_else(|| std::path::Path::new("."));

        let mut executor = BabelExecutor::new();
        executor.timeout_secs = self.babel_timeout;

        let result = executor.execute_block(&block, buf_dir, &resolved_vars);

        // Format results
        let output_text = match &result {
            babel::execute::ExecResult::Output(s) => s.clone(),
            babel::execute::ExecResult::Value(s) => s.clone(),
            babel::execute::ExecResult::File(p) => format!("[[file:{}]]", p.display()),
            babel::execute::ExecResult::Error(e) => {
                self.set_status(format!("Babel error: {}", e));
                return;
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
        self.set_status(format!("Executed {} block", block.language));
    }

    /// Execute all source blocks in the current buffer.
    /// Uses AI-aware buffer targeting.
    pub fn babel_execute_all(&mut self) {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();
        let blocks = babel::parse_src_blocks(&source);

        if blocks.is_empty() {
            self.set_status("No source blocks in buffer");
            return;
        }

        let count = blocks.len();
        // Execute blocks in reverse order to preserve line offsets
        for i in (0..count).rev() {
            // Re-read source after each edit
            let current_source = self.buffers[buf_idx].rope().to_string();
            let current_blocks = babel::parse_src_blocks(&current_source);
            if i >= current_blocks.len() {
                continue;
            }
            let block = &current_blocks[i];
            if block.header_args.eval == babel::EvalPolicy::Never {
                continue;
            }

            let file_path = self.buffers[buf_idx].file_path().map(PathBuf::from);
            let buf_dir = file_path
                .as_ref()
                .and_then(|p| p.parent())
                .unwrap_or_else(|| std::path::Path::new("."));

            let resolved_vars = babel::vars::resolve_vars(block, &current_blocks, &current_source);
            let mut executor = BabelExecutor::new();
            executor.timeout_secs = self.babel_timeout;

            let result = executor.execute_block(block, buf_dir, &resolved_vars);
            let output_text = match &result {
                babel::execute::ExecResult::Output(s) => s.clone(),
                babel::execute::ExecResult::Value(s) => s.clone(),
                babel::execute::ExecResult::File(p) => format!("[[file:{}]]", p.display()),
                babel::execute::ExecResult::Error(_) => continue,
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
        }

        self.clamp_all_cursors();
        self.mark_full_redraw();
        self.set_status(format!("Executed {} block(s)", count));
    }

    /// Tangle all source blocks in the current buffer.
    /// Uses AI-aware buffer targeting.
    pub fn babel_tangle(&mut self) {
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
            self.set_status("No blocks with :tangle directive");
            return;
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

        if errors.is_empty() {
            self.set_status(format!("Tangled {} file(s)", success_count));
        } else {
            self.set_status(format!(
                "Tangled {} file(s), {} error(s): {}",
                success_count,
                errors.len(),
                errors[0]
            ));
        }
    }

    /// Kill all babel session processes.
    pub fn babel_kill_sessions(&mut self) {
        // Sessions not yet persisted on editor — this is a placeholder
        self.set_status("No active babel sessions");
    }

    /// Export current org buffer to HTML.
    pub fn org_export_html(&mut self) {
        self.org_export_to("html");
    }

    /// Export current org buffer to Markdown.
    pub fn org_export_markdown(&mut self) {
        self.org_export_to("markdown");
    }

    /// Export subtree at cursor.
    /// Uses AI-aware buffer/cursor targeting.
    pub fn org_export_subtree(&mut self) {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();
        let cursor_line = self.ai_cursor_row();

        let (meta, elements) = export::parse_org_document(&source);

        // Find the heading at or before cursor
        let mut heading_idx = None;
        for (current_line, (i, _el)) in elements.iter().enumerate().enumerate() {
            if current_line > cursor_line {
                break;
            }
            if matches!(&elements[i], export::OrgElement::Heading { .. }) {
                heading_idx = Some(i);
            }
        }

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
    fn org_export_to(&mut self, format: &str) {
        let buf_idx = self.ai_active_buffer_idx();
        let source = self.buffers[buf_idx].rope().to_string();
        let (meta, elements) = export::parse_org_document(&source);

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
                self.set_status(format!("Unknown export format: {}", format));
                return;
            }
        };

        // Write to file alongside the org file
        let file_path = self.buffers[buf_idx].file_path().map(PathBuf::from);
        let output_path = file_path
            .as_ref()
            .map(|p| p.with_extension(ext))
            .unwrap_or_else(|| PathBuf::from(format!("export.{}", ext)));

        match std::fs::write(&output_path, &output) {
            Ok(()) => {
                self.set_status(format!("Exported to {}", output_path.display()));
            }
            Err(e) => {
                self.set_status(format!("Export failed: {}", e));
            }
        }
    }

    /// List KB instances — returns structured info for AI tools.
    pub fn kb_instances(&mut self) -> String {
        if self.kb_registry.instances.is_empty() {
            let msg = "KB federation: built-in KB only (no external instances registered)";
            self.set_status(msg);
            return msg.to_string();
        }

        let mut lines = vec![format!(
            "KB federation: {} instance(s)",
            self.kb_registry.instances.len()
        )];
        for inst in &self.kb_registry.instances {
            let count = self
                .kb_instances
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
