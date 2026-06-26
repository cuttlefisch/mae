use crate::agenda_view::{AgendaFilter, AgendaLine, AgendaLineKind, AgendaView};
use crate::buffer::{Buffer, BufferKind};
use crate::buffer_view::BufferView;

use super::Editor;

impl Editor {
    /// Open or refresh the `*Agenda*` buffer with TODO items from the KB.
    pub fn open_agenda(&mut self, filter: AgendaFilter) {
        let view = self.build_agenda_view(&filter);
        let text = render_agenda_text(&view);

        // Find or create the *Agenda* buffer.
        let buf_idx = if let Some(idx) = self.find_buffer_by_name("*Agenda*") {
            idx
        } else {
            let mut buf = Buffer::new();
            buf.name = "*Agenda*".to_string();
            buf.kind = BufferKind::Agenda;
            buf.read_only = true;
            self.buffers.push(buf);
            self.buffers.len() - 1
        };

        let buf = &mut self.buffers[buf_idx];
        buf.kind = BufferKind::Agenda;
        buf.read_only = false; // temporarily writable
        buf.replace_contents(&text);
        buf.read_only = true;
        buf.modified = false;
        buf.view = BufferView::Agenda(Box::new(view));

        self.display_buffer_and_focus(buf_idx);
        self.set_mode(crate::Mode::Normal);
    }

    /// Build the agenda view from KB data using secondary indexes.
    fn build_agenda_view(&self, filter: &AgendaFilter) -> AgendaView {
        let mut lines = Vec::new();

        // Header
        lines.push(AgendaLine {
            text: "Agenda".to_string(),
            kind: AgendaLineKind::Header,
            node_id: None,
            source_file: None,
        });
        lines.push(AgendaLine {
            text: String::new(),
            kind: AgendaLineKind::Blank,
            node_id: None,
            source_file: None,
        });

        // Collect matching TODO nodes via the query layer (Phase D: so the agenda
        // works under a thin daemon-hosted mirror — the daemon's cozo serves the
        // TODO set); the in-memory KB is the fallback when there's no query layer.
        // State filtering is applied here (the query layer returns the full TODO set).
        let all_todo: Vec<mae_kb::Node> = match self.kb.query_layer() {
            Some(q) => q.todo_nodes(),
            None => self.kb.primary.todo_nodes().into_iter().cloned().collect(),
        };
        let nodes: Vec<mae_kb::Node> = all_todo
            .into_iter()
            .filter(|node| {
                if let Some(ref states) = filter.todo_states {
                    if !node.todo_state.as_ref().is_some_and(|s| states.contains(s)) {
                        return false;
                    }
                }
                matches_filter(node, filter)
            })
            .collect();

        if nodes.is_empty() {
            lines.push(AgendaLine {
                text: "  No matching TODO items found.".to_string(),
                kind: AgendaLineKind::Blank,
                node_id: None,
                source_file: None,
            });
        } else {
            for node in &nodes {
                let state = node.todo_state.as_deref().unwrap_or("TODO");
                let pri_str = node
                    .priority
                    .map(|c| format!("[#{}] ", c))
                    .unwrap_or_default();
                let tags_str = if node.tags.is_empty() {
                    String::new()
                } else {
                    format!("  :{}", node.tags.join(":"))
                };
                let line_text = format!("  {} {}{}{}", state, pri_str, node.title, tags_str);
                lines.push(AgendaLine {
                    text: line_text,
                    kind: AgendaLineKind::TodoItem {
                        state: state.to_string(),
                        priority: node.priority,
                    },
                    node_id: Some(node.id.clone()),
                    source_file: None,
                });
            }
        }

        lines.push(AgendaLine {
            text: String::new(),
            kind: AgendaLineKind::Blank,
            node_id: None,
            source_file: None,
        });
        lines.push(AgendaLine {
            text: format!("  {} items", nodes.len()),
            kind: AgendaLineKind::Blank,
            node_id: None,
            source_file: None,
        });

        AgendaView {
            lines,
            filter: filter.clone(),
        }
    }

    /// Refresh the current agenda buffer (re-query KB).
    pub fn agenda_refresh(&mut self) {
        let filter = {
            let idx = self.active_buffer_idx();
            if self.buffers[idx].kind != BufferKind::Agenda {
                return;
            }
            match &self.buffers[idx].view {
                BufferView::Agenda(v) => v.filter.clone(),
                _ => AgendaFilter::default(),
            }
        };
        self.open_agenda(filter);
        self.set_status("Agenda refreshed");
    }

    /// Jump to the source node for the agenda line under cursor.
    pub fn agenda_goto(&mut self) {
        let idx = self.active_buffer_idx();
        if self.buffers[idx].kind != BufferKind::Agenda {
            return;
        }
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let node_id = match &self.buffers[idx].view {
            BufferView::Agenda(v) => v.lines.get(cursor_row).and_then(|l| l.node_id.clone()),
            _ => None,
        };
        if let Some(id) = node_id {
            self.open_help_at(&id);
        }
    }

    /// Cycle the TODO state filter on the agenda.
    pub fn agenda_filter_todo(&mut self) {
        let idx = self.active_buffer_idx();
        if self.buffers[idx].kind != BufferKind::Agenda {
            return;
        }
        let mut filter = match &self.buffers[idx].view {
            BufferView::Agenda(v) => v.filter.clone(),
            _ => AgendaFilter::default(),
        };
        // Cycle: all → TODO only → DONE only → NEXT only → all
        let new_states = match &filter.todo_states {
            None => Some(vec!["TODO".to_string()]),
            Some(s) if s.len() == 1 && s[0] == "TODO" => Some(vec!["DONE".to_string()]),
            Some(s) if s.len() == 1 && s[0] == "DONE" => Some(vec!["NEXT".to_string()]),
            _ => None,
        };
        filter.todo_states = new_states;
        let label = filter
            .todo_states
            .as_ref()
            .map(|s| s.join(","))
            .unwrap_or_else(|| "all".to_string());
        self.open_agenda(filter);
        self.set_status(format!("Agenda filter: {}", label));
    }

    // --- Agenda file CRUD ---

    /// Add a directory or file to the agenda file list, ingest it, and refresh
    /// the agenda buffer if it's open.
    pub fn agenda_add_path(&mut self, path: &str) {
        let expanded = crate::file_picker::expand_tilde(path);
        if !self.org_agenda_files.contains(&expanded) {
            self.org_agenda_files.push(expanded.clone());
        }
        self.ingest_single_agenda_path(&expanded);
        if self.find_buffer_by_name("*Agenda*").is_some() {
            self.agenda_refresh();
        }
        self.set_status(format!("Agenda: added {}", expanded));
    }

    /// Remove a path from the agenda file list.
    pub fn agenda_remove_path(&mut self, path: &str) {
        let expanded = crate::file_picker::expand_tilde(path);
        self.org_agenda_files.retain(|p| p != &expanded);
        self.set_status(format!("Agenda: removed {}", expanded));
    }

    /// Show current agenda file paths in the status bar.
    pub fn agenda_list_paths(&mut self) {
        if self.org_agenda_files.is_empty() {
            self.set_status("No agenda files configured");
        } else {
            self.set_status(format!(
                "Agenda files: {}",
                self.org_agenda_files.join(", ")
            ));
        }
    }

    /// Re-ingest all configured agenda paths into the KB.
    pub fn ingest_agenda_files(&mut self) {
        for path in self.org_agenda_files.clone() {
            self.ingest_single_agenda_path(&path);
        }
    }

    fn ingest_single_agenda_path(&mut self, path: &str) {
        let p = std::path::Path::new(path);
        if p.is_dir() {
            self.kb.primary.ingest_org_dir(p);
        } else if p.is_file() {
            self.kb.primary.ingest_org_file(p);
        }
    }

    /// Cycle the priority filter on the agenda.
    pub fn agenda_filter_priority(&mut self) {
        let idx = self.active_buffer_idx();
        if self.buffers[idx].kind != BufferKind::Agenda {
            return;
        }
        let mut filter = match &self.buffers[idx].view {
            BufferView::Agenda(v) => v.filter.clone(),
            _ => AgendaFilter::default(),
        };
        // Cycle: all → A → B → C → all
        filter.priority = match filter.priority {
            None => Some('A'),
            Some('A') => Some('B'),
            Some('B') => Some('C'),
            _ => None,
        };
        let label = filter
            .priority
            .map(|c| format!("[#{}]", c))
            .unwrap_or_else(|| "all".to_string());
        self.open_agenda(filter);
        self.set_status(format!("Agenda priority: {}", label));
    }
}

fn matches_filter(node: &mae_kb::Node, filter: &AgendaFilter) -> bool {
    if let Some(pri) = filter.priority {
        if node.priority != Some(pri) {
            return false;
        }
    }
    if let Some(ref tag) = filter.tag {
        if !node.tags.iter().any(|t| t == tag) {
            return false;
        }
    }
    true
}

fn render_agenda_text(view: &AgendaView) -> String {
    let mut text = String::new();
    for (i, line) in view.lines.iter().enumerate() {
        text.push_str(&line.text);
        if i + 1 < view.lines.len() {
            text.push('\n');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_agenda_creates_buffer() {
        let mut editor = Editor::new();
        // Insert some TODO nodes into KB.
        editor.kb.primary.insert(
            mae_kb::Node::new("todo:1", "Fix bug", mae_kb::NodeKind::Note, "Fix the bug")
                .with_todo_state("TODO"),
        );
        editor.kb.primary.insert(
            mae_kb::Node::new(
                "todo:2",
                "Write docs",
                mae_kb::NodeKind::Note,
                "Write the docs",
            )
            .with_todo_state("DONE"),
        );
        editor.open_agenda(AgendaFilter::default());
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        assert_eq!(editor.buffers[idx].kind, BufferKind::Agenda);
        assert!(editor.buffers[idx].read_only);
        let text = editor.buffers[idx].rope().to_string();
        assert!(text.contains("Fix bug"));
        assert!(text.contains("Write docs"));
    }

    #[test]
    fn agenda_filter_by_state() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(
            mae_kb::Node::new("todo:1", "Active", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO"),
        );
        editor.kb.primary.insert(
            mae_kb::Node::new("todo:2", "Finished", mae_kb::NodeKind::Note, "")
                .with_todo_state("DONE"),
        );
        editor.open_agenda(AgendaFilter {
            todo_states: Some(vec!["TODO".to_string()]),
            ..Default::default()
        });
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let text = editor.buffers[idx].rope().to_string();
        assert!(text.contains("Active"));
        assert!(!text.contains("Finished"));
    }

    #[test]
    fn agenda_filter_by_priority() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(
            mae_kb::Node::new("todo:1", "Urgent", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO")
                .with_priority('A'),
        );
        editor.kb.primary.insert(
            mae_kb::Node::new("todo:2", "Low", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO")
                .with_priority('C'),
        );
        editor.open_agenda(AgendaFilter {
            priority: Some('A'),
            ..Default::default()
        });
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let text = editor.buffers[idx].rope().to_string();
        assert!(text.contains("Urgent"));
        assert!(!text.contains("Low"));
    }

    #[test]
    fn agenda_filter_by_tag() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(
            mae_kb::Node::new("todo:1", "Work item", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO")
                .with_tags(["work"]),
        );
        editor.kb.primary.insert(
            mae_kb::Node::new("todo:2", "Personal", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO")
                .with_tags(["home"]),
        );
        editor.open_agenda(AgendaFilter {
            tag: Some("work".to_string()),
            ..Default::default()
        });
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let text = editor.buffers[idx].rope().to_string();
        assert!(text.contains("Work item"));
        assert!(!text.contains("Personal"));
    }

    #[test]
    fn agenda_refresh_preserves_filter() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(
            mae_kb::Node::new("todo:1", "Active", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO"),
        );
        editor.open_agenda(AgendaFilter {
            todo_states: Some(vec!["TODO".to_string()]),
            ..Default::default()
        });
        // Add another TODO after opening
        editor.kb.primary.insert(
            mae_kb::Node::new("todo:2", "New task", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO"),
        );
        editor.agenda_refresh();
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let text = editor.buffers[idx].rope().to_string();
        assert!(text.contains("New task"));
    }

    #[test]
    fn agenda_empty_kb() {
        let mut editor = Editor::new();
        editor.open_agenda(AgendaFilter::default());
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let text = editor.buffers[idx].rope().to_string();
        assert!(text.contains("No matching TODO items"));
    }

    // ---- Integration tests: org file → parser → agenda ----

    const AGENDA_ORG: &str = "\
:PROPERTIES:
:ID: agenda-test-file
:END:
#+title: Test Agenda
#+filetags: :project:

* TODO [#A] Fix critical bug :backend:urgent:
:PROPERTIES:
:ID: agenda-fix-bug
:END:
Critical bug in the backend.

* DONE [#B] Write documentation :docs:
:PROPERTIES:
:ID: agenda-write-docs
:END:
Documentation is complete.

* TODO [#C] Refactor module :backend:
:PROPERTIES:
:ID: agenda-refactor
:END:
Needs cleanup.

* NEXT [#A] Deploy to staging :ops:urgent:
:PROPERTIES:
:ID: agenda-deploy
:END:
Deploy the latest build.
";

    fn ingest_org_fixture(editor: &mut Editor, content: &str) {
        for node in mae_kb::org::parse_org_multi(content) {
            editor.kb.primary.insert(node);
        }
    }

    #[test]
    fn agenda_from_org_file_shows_all_todos() {
        let mut editor = Editor::new();
        ingest_org_fixture(&mut editor, AGENDA_ORG);
        editor.open_agenda(AgendaFilter::default());
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let text = editor.buffers[idx].rope().to_string();
        assert!(
            text.contains("Fix critical bug"),
            "missing Fix critical bug"
        );
        assert!(
            text.contains("Write documentation"),
            "missing Write documentation"
        );
        assert!(text.contains("Refactor module"), "missing Refactor module");
        assert!(
            text.contains("Deploy to staging"),
            "missing Deploy to staging"
        );
        assert!(text.contains("4 items"), "expected 4 items count");
    }

    #[test]
    fn agenda_from_org_file_filters_by_state() {
        let mut editor = Editor::new();
        ingest_org_fixture(&mut editor, AGENDA_ORG);
        editor.open_agenda(AgendaFilter {
            todo_states: Some(vec!["TODO".to_string()]),
            ..Default::default()
        });
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let text = editor.buffers[idx].rope().to_string();
        assert!(text.contains("Fix critical bug"));
        assert!(text.contains("Refactor module"));
        assert!(!text.contains("Write documentation")); // DONE
        assert!(!text.contains("Deploy to staging")); // NEXT
    }

    #[test]
    fn agenda_from_org_file_filters_by_priority() {
        let mut editor = Editor::new();
        ingest_org_fixture(&mut editor, AGENDA_ORG);
        editor.open_agenda(AgendaFilter {
            priority: Some('A'),
            ..Default::default()
        });
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let text = editor.buffers[idx].rope().to_string();
        assert!(text.contains("Fix critical bug"));
        assert!(text.contains("Deploy to staging"));
        assert!(!text.contains("Write documentation"));
        assert!(!text.contains("Refactor module"));
    }

    #[test]
    fn agenda_from_org_file_filters_by_tag() {
        let mut editor = Editor::new();
        ingest_org_fixture(&mut editor, AGENDA_ORG);
        editor.open_agenda(AgendaFilter {
            tag: Some("urgent".to_string()),
            ..Default::default()
        });
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let text = editor.buffers[idx].rope().to_string();
        assert!(text.contains("Fix critical bug"));
        assert!(text.contains("Deploy to staging"));
        assert!(!text.contains("Write documentation"));
        assert!(!text.contains("Refactor module"));
    }

    #[test]
    fn agenda_from_org_file_view_structure() {
        let mut editor = Editor::new();
        ingest_org_fixture(&mut editor, AGENDA_ORG);
        editor.open_agenda(AgendaFilter::default());
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let view = match &editor.buffers[idx].view {
            BufferView::Agenda(v) => v.as_ref(),
            _ => panic!("expected Agenda view"),
        };
        // Structure: Header, Blank, 4×TodoItem, Blank, count
        assert!(matches!(view.lines[0].kind, AgendaLineKind::Header));
        assert!(matches!(view.lines[1].kind, AgendaLineKind::Blank));
        let todo_items: Vec<_> = view
            .lines
            .iter()
            .filter(|l| matches!(l.kind, AgendaLineKind::TodoItem { .. }))
            .collect();
        assert_eq!(todo_items.len(), 4, "expected 4 TodoItem lines");
        // Verify state and priority on individual items
        for item in &todo_items {
            if let AgendaLineKind::TodoItem { state, priority } = &item.kind {
                if item.text.contains("Fix critical bug") {
                    assert_eq!(state, "TODO");
                    assert_eq!(*priority, Some('A'));
                } else if item.text.contains("Deploy to staging") {
                    assert_eq!(state, "NEXT");
                    assert_eq!(*priority, Some('A'));
                }
            }
        }
    }

    // ---- Performance benchmark ----

    #[test]
    fn agenda_scales_to_1000_todos() {
        let mut editor = Editor::new();
        for i in 0..1000 {
            let pri = match i % 3 {
                0 => 'A',
                1 => 'B',
                _ => 'C',
            };
            editor.kb.primary.insert(
                mae_kb::Node::new(
                    format!("perf:{}", i),
                    format!("Task {}", i),
                    mae_kb::NodeKind::Note,
                    "",
                )
                .with_todo_state("TODO")
                .with_priority(pri),
            );
        }
        let start = std::time::Instant::now();
        editor.open_agenda(AgendaFilter::default());
        let elapsed = start.elapsed();
        let idx = editor.find_buffer_by_name("*Agenda*").unwrap();
        let text = editor.buffers[idx].rope().to_string();
        assert!(text.contains("1000 items"), "expected 1000 items");
        assert!(
            elapsed.as_millis() < 50,
            "agenda with 1000 items took {}ms (> 50ms budget)",
            elapsed.as_millis()
        );
    }

    // ---- Agenda file CRUD tests ----

    #[test]
    fn agenda_add_path_ingests_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let org_path = tmp.path().join("test.org");
        std::fs::write(
            &org_path,
            ":PROPERTIES:\n:ID: tmp-node-1\n:END:\n#+title: Tmp\n* TODO Task one\n:PROPERTIES:\n:ID: tmp-task-1\n:END:\n",
        )
        .unwrap();
        let mut editor = Editor::new();
        editor.agenda_add_path(&tmp.path().to_string_lossy());
        assert!(
            editor.kb.primary.contains("tmp-task-1"),
            "node should be ingested"
        );
        assert_eq!(editor.org_agenda_files.len(), 1);
    }

    #[test]
    fn agenda_remove_path_removes_from_list() {
        let mut editor = Editor::new();
        editor.org_agenda_files.push("/tmp/test".to_string());
        editor.agenda_remove_path("/tmp/test");
        assert!(editor.org_agenda_files.is_empty());
    }

    #[test]
    fn agenda_ingest_rescans_all_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let org_path = tmp.path().join("rescan.org");
        std::fs::write(
            &org_path,
            ":PROPERTIES:\n:ID: rescan-file\n:END:\n#+title: Rescan\n* TODO Rescan task\n:PROPERTIES:\n:ID: rescan-task-1\n:END:\n",
        )
        .unwrap();
        let mut editor = Editor::new();
        editor
            .org_agenda_files
            .push(tmp.path().to_string_lossy().to_string());
        editor.ingest_agenda_files();
        assert!(
            editor.kb.primary.contains("rescan-task-1"),
            "node should be ingested"
        );
    }
}
