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

        // Collect matching nodes from KB indexes.
        let nodes: Vec<_> = if let Some(ref states) = filter.todo_states {
            let mut result = Vec::new();
            for state in states {
                for node in self.kb.nodes_by_todo_state(state) {
                    if matches_filter(node, filter) {
                        result.push(node.clone());
                    }
                }
            }
            result
        } else {
            // All TODO nodes
            self.kb
                .todo_nodes()
                .into_iter()
                .filter(|n| matches_filter(n, filter))
                .cloned()
                .collect()
        };

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
        let mut ed = Editor::new();
        // Insert some TODO nodes into KB.
        ed.kb.insert(
            mae_kb::Node::new("todo:1", "Fix bug", mae_kb::NodeKind::Note, "Fix the bug")
                .with_todo_state("TODO"),
        );
        ed.kb.insert(
            mae_kb::Node::new(
                "todo:2",
                "Write docs",
                mae_kb::NodeKind::Note,
                "Write the docs",
            )
            .with_todo_state("DONE"),
        );
        ed.open_agenda(AgendaFilter::default());
        let idx = ed.find_buffer_by_name("*Agenda*").unwrap();
        assert_eq!(ed.buffers[idx].kind, BufferKind::Agenda);
        assert!(ed.buffers[idx].read_only);
        let text = ed.buffers[idx].rope().to_string();
        assert!(text.contains("Fix bug"));
        assert!(text.contains("Write docs"));
    }

    #[test]
    fn agenda_filter_by_state() {
        let mut ed = Editor::new();
        ed.kb.insert(
            mae_kb::Node::new("todo:1", "Active", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO"),
        );
        ed.kb.insert(
            mae_kb::Node::new("todo:2", "Finished", mae_kb::NodeKind::Note, "")
                .with_todo_state("DONE"),
        );
        ed.open_agenda(AgendaFilter {
            todo_states: Some(vec!["TODO".to_string()]),
            ..Default::default()
        });
        let idx = ed.find_buffer_by_name("*Agenda*").unwrap();
        let text = ed.buffers[idx].rope().to_string();
        assert!(text.contains("Active"));
        assert!(!text.contains("Finished"));
    }

    #[test]
    fn agenda_filter_by_priority() {
        let mut ed = Editor::new();
        ed.kb.insert(
            mae_kb::Node::new("todo:1", "Urgent", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO")
                .with_priority('A'),
        );
        ed.kb.insert(
            mae_kb::Node::new("todo:2", "Low", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO")
                .with_priority('C'),
        );
        ed.open_agenda(AgendaFilter {
            priority: Some('A'),
            ..Default::default()
        });
        let idx = ed.find_buffer_by_name("*Agenda*").unwrap();
        let text = ed.buffers[idx].rope().to_string();
        assert!(text.contains("Urgent"));
        assert!(!text.contains("Low"));
    }

    #[test]
    fn agenda_filter_by_tag() {
        let mut ed = Editor::new();
        ed.kb.insert(
            mae_kb::Node::new("todo:1", "Work item", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO")
                .with_tags(["work"]),
        );
        ed.kb.insert(
            mae_kb::Node::new("todo:2", "Personal", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO")
                .with_tags(["home"]),
        );
        ed.open_agenda(AgendaFilter {
            tag: Some("work".to_string()),
            ..Default::default()
        });
        let idx = ed.find_buffer_by_name("*Agenda*").unwrap();
        let text = ed.buffers[idx].rope().to_string();
        assert!(text.contains("Work item"));
        assert!(!text.contains("Personal"));
    }

    #[test]
    fn agenda_refresh_preserves_filter() {
        let mut ed = Editor::new();
        ed.kb.insert(
            mae_kb::Node::new("todo:1", "Active", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO"),
        );
        ed.open_agenda(AgendaFilter {
            todo_states: Some(vec!["TODO".to_string()]),
            ..Default::default()
        });
        // Add another TODO after opening
        ed.kb.insert(
            mae_kb::Node::new("todo:2", "New task", mae_kb::NodeKind::Note, "")
                .with_todo_state("TODO"),
        );
        ed.agenda_refresh();
        let idx = ed.find_buffer_by_name("*Agenda*").unwrap();
        let text = ed.buffers[idx].rope().to_string();
        assert!(text.contains("New task"));
    }

    #[test]
    fn agenda_empty_kb() {
        let mut ed = Editor::new();
        ed.open_agenda(AgendaFilter::default());
        let idx = ed.find_buffer_by_name("*Agenda*").unwrap();
        let text = ed.buffers[idx].rope().to_string();
        assert!(text.contains("No matching TODO items"));
    }
}
