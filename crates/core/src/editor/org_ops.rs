//! Org-mode specific operations: cycle, TODO, priority, tags, links, export.

use super::Editor;
use tracing::info;

impl Editor {
    /// Three-state org heading cycle (TAB).
    ///
    /// Cycle: SUBTREE (all visible) → FOLDED (heading only) → CHILDREN
    /// (body + child headings visible, child bodies folded) → SUBTREE.
    /// Leaf headings (no children) cycle: SUBTREE ↔ FOLDED.
    pub fn org_cycle(&mut self) -> String {
        let buf_idx = self.active_buffer_idx();
        let lang = self.syntax.language_of(buf_idx);
        if lang != Some(crate::syntax::Language::Org) {
            return "Not an org buffer".to_string();
        }
        // Tab on a table line → cell navigation instead of heading fold.
        let row = self.window_mgr.focused_window().cursor_row;
        if crate::table::table_at_line(self.buffers[buf_idx].rope(), row).is_some() {
            self.table_next_cell();
            // `table_next_cell` sets its own status; no intervening call
            // between it and this read, so (unlike #304's cross-boundary
            // bug) there's no clobber risk here.
            return self.status_msg.clone();
        }
        self.heading_cycle(crate::syntax::Language::Org);
        self.status_msg.clone()
    }

    /// Cycle TODO state for org/markdown headings: none→TODO→DONE→TODO.
    pub fn org_todo_cycle(&mut self) -> String {
        let buf_idx = self.active_buffer_idx();

        info!(buf_idx, "org todo cycle");
        let row = self.window_mgr.focused_window().cursor_row;
        let line = self.buffers[buf_idx].rope().line(row);
        let line_str: String = line.chars().collect();

        let (new_line, status) = if line_str.contains("TODO ") {
            (line_str.replacen("TODO ", "DONE ", 1), "DONE")
        } else if line_str.contains("DONE ") {
            (line_str.replacen("DONE ", "TODO ", 1), "TODO")
        } else {
            // Find the start of the heading text (after stars or hashes)
            let mut prefix_end = 0;
            let mut found = false;
            for (i, ch) in line_str.chars().enumerate() {
                if ch == '*' || ch == '#' {
                    found = true;
                } else if found && ch == ' ' {
                    prefix_end = i + 1;
                    break;
                } else {
                    break;
                }
            }

            if found && prefix_end > 0 {
                let mut next = line_str.clone();
                next.insert_str(prefix_end, "TODO ");
                (next, "TODO")
            } else {
                (line_str.clone(), "Not a heading")
            }
        };

        if new_line != line_str {
            let start = self.buffers[buf_idx].rope().line_to_char(row);
            let end = start + line.len_chars();
            self.buffers[buf_idx].begin_undo_group();
            self.buffers[buf_idx].delete_range(start, end);
            self.buffers[buf_idx].insert_text_at(start, &new_line);
            self.buffers[buf_idx].end_undo_group();
            self.set_status(status);
            // Update parent heading's statistics cookies ([/] or [%])
            self.update_statistics_cookies(row);
        }
        // Previously `set_status` (and thus the returned status) was only
        // reached inside the `if new_line != line_str` block above, so a
        // non-heading line silently returned whatever status happened to be
        // set before this call (#304's exact anti-pattern, one level up) —
        // returning `status` unconditionally fixes that too, not just the
        // wrapper-level staleness.
        status.to_string()
    }

    /// Promote Org heading (thin wrapper).
    pub fn org_promote(&mut self) {
        self.heading_promote(crate::syntax::Language::Org);
    }

    /// Demote Org heading (thin wrapper).
    pub fn org_demote(&mut self) {
        self.heading_demote(crate::syntax::Language::Org);
    }

    /// Cycle org heading priority up: none → [#A] → [#B] → [#C] → none.
    pub fn org_priority_up(&mut self) {
        self.org_priority_cycle(true);
    }

    /// Cycle org heading priority down: none → [#C] → [#B] → [#A] → none.
    pub fn org_priority_down(&mut self) {
        self.org_priority_cycle(false);
    }

    fn org_priority_cycle(&mut self, up: bool) {
        use regex::Regex;
        use std::sync::OnceLock;

        static HEADING_PRI: OnceLock<Regex> = OnceLock::new();
        let re = HEADING_PRI.get_or_init(|| {
            Regex::new(
                r"^(\*+ )(?:(TODO|DONE|NEXT|WAIT|CANCELLED|DEFERRED) )?(?:\[#([A-C])\] )?(.*)",
            )
            .unwrap()
        });

        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
        let Some(cap) = re.captures(&line) else {
            self.set_status("Not a heading");
            return;
        };

        let stars = cap.get(1).unwrap().as_str();
        let keyword = cap.get(2).map(|m| m.as_str());
        let current_pri = cap.get(3).map(|m| m.as_str());
        let rest = cap.get(4).map(|m| m.as_str()).unwrap_or("");

        let new_pri = if up {
            match current_pri {
                None => Some("A"),
                Some("A") => Some("B"),
                Some("B") => Some("C"),
                _ => None,
            }
        } else {
            match current_pri {
                None => Some("C"),
                Some("C") => Some("B"),
                Some("B") => Some("A"),
                _ => None,
            }
        };

        let mut new_line = String::from(stars);
        if let Some(kw) = keyword {
            new_line.push_str(kw);
            new_line.push(' ');
        }
        if let Some(pri) = new_pri {
            new_line.push_str(&format!("[#{}] ", pri));
        }
        new_line.push_str(rest);
        // Preserve trailing newline
        if line.ends_with('\n') && !new_line.ends_with('\n') {
            new_line.push('\n');
        }

        let start = self.buffers[buf_idx].rope().line_to_char(row);
        let end = start + self.buffers[buf_idx].rope().line(row).len_chars();
        self.buffers[buf_idx].begin_undo_group();
        self.buffers[buf_idx].delete_range(start, end);
        self.buffers[buf_idx].insert_text_at(start, &new_line);
        self.buffers[buf_idx].end_undo_group();
        let label = new_pri
            .map(|p| format!("[#{}]", p))
            .unwrap_or_else(|| "none".into());
        self.set_status(format!("Priority: {}", label));
    }

    /// Open a MiniDialog to set tags on the current org heading.
    pub fn org_set_tags(&mut self) {
        use crate::command_palette::{MiniDialogContext, MiniDialogState};

        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();

        if !line.trim_start().starts_with('*') {
            self.set_status("Not a heading");
            return;
        }

        // Extract current tags from trailing :tag1:tag2: pattern.
        let trimmed = line.trim_end_matches('\n').trim_end();
        let current_tags = if let Some(last_space) = trimmed.rfind(char::is_whitespace) {
            let tail = &trimmed[last_space + 1..];
            if tail.starts_with(':') && tail.ends_with(':') && tail.len() >= 3 {
                tail[1..tail.len() - 1].to_string()
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        self.mini_dialog = Some(MiniDialogState::single_input(
            "Tags (colon-separated)",
            &current_tags,
            "tag1:tag2",
            MiniDialogContext::OrgSetTags { heading_line: row },
        ));
        self.set_mode(crate::Mode::CommandPalette);
    }

    /// Calculate the range of lines covered by the subtree rooted at the
    /// heading on `row`. Returns `(start_row, end_row_exclusive)` where
    /// `start_row` is the heading itself and `end_row_exclusive` is the
    /// first line of the next sibling (same or higher level) or EOF.
    pub fn org_subtree_range(&self, row: usize) -> Option<(usize, usize)> {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(crate::syntax::Language::Org) {
            return None;
        }
        self.heading_subtree_range(row, crate::syntax::Language::Org)
    }

    /// Move org subtree down (thin wrapper).
    pub fn org_move_subtree_down(&mut self) {
        self.heading_move_subtree_down(crate::syntax::Language::Org);
    }

    /// Move org subtree up (thin wrapper).
    pub fn org_move_subtree_up(&mut self) {
        self.heading_move_subtree_up(crate::syntax::Language::Org);
    }

    /// Open the Org link at the cursor.
    ///
    /// Fixed in #306: this used to look up the link via
    /// `self.syntax.tree_for`, but `Language::Org::ts_language()`
    /// (`syntax/languages.rs`) deliberately `return None`s — "org is
    /// highlighted through a fallback path," no tree-sitter grammar exists
    /// for it in this codebase at all — so the lookup always failed and
    /// this function was unconditionally dead code. Now uses
    /// `link_detect::detect_org_links`, the same org-bracket-link scanner
    /// the interactive concealment pipeline (`link_descriptive`,
    /// `display_region::compute_link_regions`) already relies on, instead
    /// of a grammar that was never wired up.
    pub fn org_open_link(&mut self) -> String {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(crate::syntax::Language::Org) {
            return "Not an org buffer".to_string();
        }

        info!(buf_idx, "org open link at cursor");

        let win = self.window_mgr.focused_window();
        let cursor_char = self.buffers[buf_idx].char_offset_at(win.cursor_row, win.cursor_col);
        let cursor_byte = self.buffers[buf_idx].rope().char_to_byte(cursor_char);

        let source: String = self.buffers[buf_idx].rope().chars().collect();
        let links = crate::link_detect::detect_org_links(&source);
        let Some(link) = links
            .iter()
            .find(|l| cursor_byte >= l.byte_start && cursor_byte < l.byte_end)
        else {
            return "No link under cursor".to_string();
        };
        let target = link.target.trim();
        if target.starts_with("http") {
            // Open external link
            let _ = std::process::Command::new(crate::link_detect::browser_command())
                .arg(target)
                .spawn();
            let msg = format!("Opening {}", target);
            self.set_status(msg.clone());
            msg
        } else if self.resolve_kb_link(target) {
            // #293/#304: KB-shaped targets (id:UUID, namespaced ids like
            // daily:2026-07-06) route through the same resolver
            // handle_link_click uses, instead of this function's own
            // separate, non-KB-aware fallback below ever seeing them.
            self.status_msg.clone()
        } else {
            // Jump to internal heading — search buffer for matching heading
            let buf = self.active_buffer();
            let target_lower = target.to_lowercase();
            let mut found = false;
            for line_idx in 0..buf.line_count() {
                let line = buf.line_text(line_idx);
                let trimmed = line.trim_start_matches('*').trim_start();
                if trimmed.to_lowercase().starts_with(&target_lower) {
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = line_idx;
                    win.cursor_col = 0;
                    found = true;
                    break;
                }
            }
            let msg = if found {
                format!("Jumped to: {}", target)
            } else {
                format!("Heading not found: {}", target)
            };
            self.set_status(msg.clone());
            msg
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Language;

    fn org_editor(text: &str) -> Editor {
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, text);
        editor.syntax.set_language(0, Language::Org);
        editor
    }

    #[test]
    fn org_demote_adds_star() {
        let mut editor = org_editor("* Heading\nBody\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_demote();
        assert_eq!(editor.buffers[0].text(), "** Heading\nBody\n");
        assert!(editor.status_msg.contains("level 2"));
    }

    #[test]
    fn org_promote_removes_star() {
        let mut editor = org_editor("** Heading\nBody\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_promote();
        assert_eq!(editor.buffers[0].text(), "* Heading\nBody\n");
        assert!(editor.status_msg.contains("level 1"));
    }

    #[test]
    fn org_promote_single_star_noop() {
        let mut editor = org_editor("* Heading\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_promote();
        assert_eq!(editor.buffers[0].text(), "* Heading\n");
    }

    // --- #306: org_open_link, now that its link lookup actually works ---

    #[test]
    fn org_open_link_resolves_kb_node_via_shared_resolver() {
        let mut editor = org_editor("See [[user:link-target][Target]] here.\n");
        editor.kb.primary.insert(mae_kb::Node::new(
            "user:link-target",
            "Link Target",
            mae_kb::NodeKind::Note,
            "body",
        ));
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.window_mgr.focused_window_mut().cursor_col = 6;
        editor.org_open_link();
        assert_eq!(
            editor.buffers[editor.active_buffer_idx()].kind,
            crate::BufferKind::Kb,
            "a KB-shaped org link must resolve through the shared KB resolver, \
             not fall through to the heading-jump fallback"
        );
        assert_eq!(editor.kb_view().unwrap().current, "user:link-target");
    }

    #[test]
    fn org_open_link_falls_back_to_heading_jump_for_non_kb_target() {
        let mut editor = org_editor("* Some Heading\nSee [[Some Heading][link]] here.\n");
        editor.window_mgr.focused_window_mut().cursor_row = 1;
        editor.window_mgr.focused_window_mut().cursor_col = 6;
        let msg = editor.org_open_link();
        assert_eq!(
            editor.window_mgr.focused_window().cursor_row,
            0,
            "a non-KB link target must fall through to the same-buffer heading-jump"
        );
        assert!(msg.contains("Jumped to"), "got: {}", msg);
    }

    #[test]
    fn org_open_link_no_link_under_cursor() {
        let mut editor = org_editor("no links on this line\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.window_mgr.focused_window_mut().cursor_col = 3;
        let msg = editor.org_open_link();
        assert_eq!(msg, "No link under cursor");
    }

    #[test]
    fn dedent_line_dispatches_org_promote() {
        let mut editor = org_editor("** Heading\nBody\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.dispatch_builtin("dedent-line");
        assert_eq!(editor.buffers[0].text(), "* Heading\nBody\n");
    }

    #[test]
    fn indent_line_dispatches_org_demote() {
        let mut editor = org_editor("* Heading\nBody\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.dispatch_builtin("indent-line");
        assert_eq!(editor.buffers[0].text(), "** Heading\nBody\n");
    }

    #[test]
    fn org_demote_non_heading_noop() {
        let mut editor = org_editor("Just text\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_demote();
        assert_eq!(editor.buffers[0].text(), "Just text\n");
    }

    #[test]
    fn org_subtree_range_single() {
        let editor = org_editor("* H1\nBody\n* H2\n");
        let range = editor.org_subtree_range(0);
        assert_eq!(range, Some((0, 2)));
    }

    #[test]
    fn org_subtree_range_nested() {
        let editor = org_editor("* H1\n** Sub\nBody\n* H2\n");
        let range = editor.org_subtree_range(0);
        assert_eq!(range, Some((0, 3)));
        let range = editor.org_subtree_range(1);
        assert_eq!(range, Some((1, 3)));
    }

    #[test]
    fn org_move_subtree_down() {
        let mut editor = org_editor("* H1\nBody1\n* H2\nBody2\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_move_subtree_down();
        assert_eq!(editor.buffers[0].text(), "* H2\nBody2\n* H1\nBody1\n");
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
    }

    #[test]
    fn org_move_subtree_up() {
        let mut editor = org_editor("* H1\nBody1\n* H2\nBody2\n");
        editor.window_mgr.focused_window_mut().cursor_row = 2;
        editor.org_move_subtree_up();
        assert_eq!(editor.buffers[0].text(), "* H2\nBody2\n* H1\nBody1\n");
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);
    }

    #[test]
    fn org_move_at_boundary_noop() {
        let mut editor = org_editor("* H1\nBody\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_move_subtree_down();
        assert_eq!(editor.buffers[0].text(), "* H1\nBody\n");
        editor.org_move_subtree_up();
        assert_eq!(editor.buffers[0].text(), "* H1\nBody\n");
    }

    // --- Three-state org heading cycle tests ---

    #[test]
    fn org_cycle_subtree_to_folded() {
        let mut editor = org_editor("* H1\nBody\n** Sub\nSub body\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_cycle();
        assert!(
            editor.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0),
            "Expected fold at row 0"
        );
        assert!(editor.status_msg.contains("Folded"));
    }

    #[test]
    fn org_cycle_folded_to_children() {
        let mut editor = org_editor("* H1\nBody\n** Sub\nSub body\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        // First TAB: SUBTREE → FOLDED
        editor.org_cycle();
        assert!(editor.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0));
        // Second TAB: FOLDED → CHILDREN
        editor.org_cycle();
        assert!(
            !editor.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0),
            "Heading 0 should not be folded in CHILDREN state"
        );
        // Child heading at row 2 should be folded
        assert!(
            editor.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 2),
            "Child heading at row 2 should be folded"
        );
        assert!(editor.status_msg.contains("Children"));
    }

    #[test]
    fn org_cycle_children_to_subtree() {
        let mut editor = org_editor("* H1\nBody\n** Sub\nSub body\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_cycle(); // SUBTREE → FOLDED
        editor.org_cycle(); // FOLDED → CHILDREN
        editor.org_cycle(); // CHILDREN → SUBTREE
        assert!(
            editor.buffers[0].folded_ranges.is_empty(),
            "All folds should be cleared in SUBTREE state"
        );
        assert!(editor.status_msg.contains("Subtree"));
    }

    #[test]
    fn org_cycle_full_round_trip() {
        let mut editor = org_editor("* H1\nBody\n** Sub\nSub body\n* H2\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        assert!(editor.buffers[0].folded_ranges.is_empty());
        editor.org_cycle(); // → FOLDED
        editor.org_cycle(); // → CHILDREN
        editor.org_cycle(); // → SUBTREE
        assert!(editor.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn org_cycle_leaf_heading_two_state() {
        let mut editor = org_editor("* H1\nBody line 1\nBody line 2\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_cycle(); // → FOLDED
        assert!(!editor.buffers[0].folded_ranges.is_empty());
        editor.org_cycle(); // → UNFOLDED
        assert!(editor.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn org_cycle_nested_children() {
        let mut editor = org_editor("* H1\n** Sub1\n*** Deep\nDeep body\n** Sub2\nSub2 body\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_cycle(); // → FOLDED
        editor.org_cycle(); // → CHILDREN
                            // ** Sub1 (row 1) should be folded (has content below)
        assert!(
            editor.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 1),
            "Sub1 should be folded in CHILDREN state"
        );
        // ** Sub2 (row 4) should be folded
        assert!(
            editor.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 4),
            "Sub2 should be folded in CHILDREN state"
        );
    }

    // --- Fold-aware structural editing tests ---

    #[test]
    fn org_move_subtree_down_clears_folds() {
        let mut editor = org_editor("* H1\nBody1\n* H2\nBody2\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.buffers[0].folded_ranges.push((0, 2));
        editor.org_move_subtree_down();
        assert!(
            editor.buffers[0].folded_ranges.is_empty(),
            "Folds should be cleared after move: {:?}",
            editor.buffers[0].folded_ranges
        );
    }

    #[test]
    fn org_move_subtree_up_clears_folds() {
        let mut editor = org_editor("* H1\nBody1\n* H2\nBody2\n");
        editor.window_mgr.focused_window_mut().cursor_row = 2;
        editor.buffers[0].folded_ranges.push((2, 4));
        editor.org_move_subtree_up();
        assert!(
            editor.buffers[0].folded_ranges.is_empty(),
            "Folds should be cleared after move up"
        );
    }

    #[test]
    fn org_promote_preserves_folds() {
        let mut editor = org_editor("** Heading\nBody\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.buffers[0].folded_ranges.push((0, 2));
        editor.org_promote();
        assert_eq!(
            editor.buffers[0].folded_ranges.len(),
            1,
            "Promote should preserve folds"
        );
    }

    #[test]
    fn org_demote_preserves_folds() {
        let mut editor = org_editor("* Heading\nBody\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.buffers[0].folded_ranges.push((0, 2));
        editor.org_demote();
        assert_eq!(
            editor.buffers[0].folded_ranges.len(),
            1,
            "Demote should preserve folds"
        );
    }

    // --- heading_level helper tests ---

    #[test]
    fn heading_level_org() {
        assert_eq!(Editor::heading_level("* H1", Language::Org), 1);
        assert_eq!(Editor::heading_level("** H2", Language::Org), 2);
        assert_eq!(Editor::heading_level("*** H3", Language::Org), 3);
        assert_eq!(Editor::heading_level("Not a heading", Language::Org), 0);
        assert_eq!(Editor::heading_level("**nospace", Language::Org), 0);
    }

    #[test]
    fn heading_scale_option_toggle() {
        let mut editor = Editor::new();
        assert!(editor.heading_scale); // default on
        assert!(editor.set_option("heading_scale", "false").is_ok());
        assert!(!editor.heading_scale);
        assert!(editor.set_option("heading-scale", "true").is_ok());
        assert!(editor.heading_scale);
    }

    // --- zM/zR for org headings ---

    #[test]
    fn org_close_all_folds() {
        let mut editor = org_editor("* H1\nBody1\n* H2\nBody2\n");
        editor.close_all_folds();
        assert!(!editor.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn org_open_all_folds_clears() {
        let mut editor = org_editor("* H1\nBody1\n* H2\nBody2\n");
        editor.close_all_folds();
        editor.open_all_folds();
        assert!(editor.buffers[0].folded_ranges.is_empty());
    }

    // --- TODO cycle tests ---

    #[test]
    fn todo_cycle_adds_todo() {
        let mut editor = org_editor("* Heading\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_todo_cycle();
        assert!(editor.buffers[0].text().contains("TODO"));
    }

    #[test]
    fn todo_cycle_todo_to_done() {
        let mut editor = org_editor("* TODO Heading\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_todo_cycle();
        assert!(editor.buffers[0].text().contains("DONE"));
        assert!(!editor.buffers[0].text().contains("TODO"));
    }

    #[test]
    fn todo_cycle_done_to_todo() {
        let mut editor = org_editor("* DONE Heading\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.org_todo_cycle();
        assert!(editor.buffers[0].text().contains("TODO"));
        assert!(!editor.buffers[0].text().contains("DONE"));
    }
}
