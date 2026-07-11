//! Graph KB dispatch helpers: agenda/history/restore/raw-query display,
//! and meta-node narrow/widen editing.

use super::*;

impl Editor {
    /// Show text content in a read-only scratch buffer.
    pub(super) fn show_scratch_buffer(&mut self, name: &str, content: &str) {
        let mut buf = crate::buffer::Buffer::new();
        buf.name = name.to_string();
        buf.replace_contents(content);
        buf.modified = false;
        buf.read_only = true;
        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    /// Dispatch `:kb-agenda` with a filter string.
    pub fn dispatch_kb_agenda(&mut self, input: &str) {
        use mae_kb::AgendaFilter;

        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let filter = match parts[0] {
            "todo" => AgendaFilter::Todo(parts.get(1).map(|s| s.trim().to_string())),
            "priority" => {
                let ch = parts
                    .get(1)
                    .and_then(|s| s.trim().chars().next())
                    .unwrap_or('A');
                AgendaFilter::Priority(ch)
            }
            "tag" => match parts.get(1) {
                Some(t) => AgendaFilter::Tag(t.trim().to_string()),
                None => {
                    self.set_status("Usage: :kb-agenda tag <TAG>");
                    return;
                }
            },
            "orphan" => AgendaFilter::Orphan,
            "dead-end" | "deadend" => AgendaFilter::DeadEnd,
            "stale" => {
                let days = parts
                    .get(1)
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(30);
                AgendaFilter::Stale(days)
            }
            "custom" => match parts.get(1) {
                Some(q) => AgendaFilter::Custom(q.trim().to_string()),
                None => {
                    self.set_status("Usage: :kb-agenda custom <datalog-query>");
                    return;
                }
            },
            other => {
                self.set_status(format!(
                    "Unknown filter '{}'. Use: todo, priority, tag, orphan, dead-end, stale, custom",
                    other
                ));
                return;
            }
        };

        // Phase 3: route the agenda through the query layer so it resolves uniformly
        // in BOTH modes (daemon-less → local cozo store; daemon-hosted → daemon read
        // layer, closing part of the #118 thin-client gap). Fall back to the primary
        // store directly if no query layer is built yet.
        let nodes = if let Some(q) = self.kb.query_layer() {
            q.agenda(&filter)
        } else if let Some(ref store) = self.kb.store {
            store.agenda_query(&filter).unwrap_or_default()
        } else {
            self.set_status("No persistent KB store (CozoDB required)");
            return;
        };

        let mut lines = Vec::new();
        lines.push(format!("KB Agenda: {} results", nodes.len()));
        lines.push("=".repeat(40));
        lines.push(String::new());
        for node in &nodes {
            let todo = match &node.todo_state {
                Some(s) if !s.is_empty() => format!(" [{}]", s),
                _ => String::new(),
            };
            let prio = match node.priority {
                Some(c) => format!(" #{}", c),
                None => String::new(),
            };
            lines.push(format!("  {}{}{} — {}", node.id, todo, prio, node.title));
        }
        if nodes.is_empty() {
            lines.push("  (no matching nodes)".to_string());
        }
        self.show_scratch_buffer("*KB Agenda*", &lines.join("\n"));
    }

    /// Dispatch `:kb-history <node-id>`.
    pub fn dispatch_kb_history(&mut self, id: &str) {
        // Phase 3: route history through the query layer (uniform in both modes),
        // falling back to the primary store directly if no query layer is built.
        let versions = if let Some(q) = self.kb.query_layer() {
            q.history(id, 50)
        } else if let Some(ref store) = self.kb.store {
            store.node_history(id, 50).unwrap_or_default()
        } else {
            self.set_status("No persistent KB store (CozoDB required)");
            return;
        };

        let mut lines = Vec::new();
        lines.push(format!(
            "Version History: {} ({} versions)",
            id,
            versions.len()
        ));
        lines.push("=".repeat(50));
        lines.push(String::new());
        for v in &versions {
            let ts = if v.created_at > 0 {
                format!(" @{}", v.created_at)
            } else {
                String::new()
            };
            lines.push(format!(
                "  v{}: {} [{}]{} — {}",
                v.version, v.title, v.author, ts, v.change_summary
            ));
        }
        if versions.is_empty() {
            lines.push("  (no version history)".to_string());
        }
        self.show_scratch_buffer("*KB History*", &lines.join("\n"));
    }

    /// Dispatch `:kb-restore <node-id> <version>`.
    pub fn dispatch_kb_restore(&mut self, id: &str, version: i64) {
        let store = match &self.kb.store {
            Some(s) => s.clone(),
            None => {
                self.set_status("No persistent KB store (CozoDB required)");
                return;
            }
        };

        match store.restore_version(id, version) {
            Ok(()) => {
                self.set_status(format!("Restored '{}' to version {}", id, version));
            }
            Err(e) => {
                self.set_status(format!("Restore failed: {}", e));
            }
        }
    }

    /// Dispatch `:kb-raw-query <datalog>`.
    pub fn dispatch_kb_raw_query(&mut self, query: &str) {
        let store = match &self.kb.store {
            Some(s) => s.clone(),
            None => {
                self.set_status("No persistent KB store (CozoDB required)");
                return;
            }
        };

        match store.raw_query(query) {
            Ok((headers, rows)) => {
                let mut lines = Vec::new();
                lines.push(format!("Datalog Query Results ({} rows)", rows.len()));
                lines.push("=".repeat(50));
                lines.push(String::new());

                if !headers.is_empty() {
                    lines.push(format!("  {}", headers.join(" | ")));
                    lines.push(format!("  {}", "-".repeat(headers.len() * 15)));
                }

                for row in &rows {
                    lines.push(format!("  {}", row.join(" | ")));
                }
                if rows.is_empty() {
                    lines.push("  (no results)".to_string());
                }
                self.show_scratch_buffer("*KB Query*", &lines.join("\n"));
            }
            Err(e) => {
                self.set_status(format!("Query failed: {}", e));
            }
        }
    }

    // --- Meta-node narrow/widen editing (Phase 7) ---

    /// Narrow to a meta-node component for editing.
    ///
    /// If the current help buffer shows a meta-node, presents its members
    /// for selection. On selection, opens the member node's body in a
    /// new buffer for editing.
    pub fn kb_narrow_meta(&mut self) {
        // Get current KB view's node ID.
        let node_id = match self.buffers[self.active_buffer_idx()].kb_view() {
            Some(hv) => hv.current.clone(),
            None => {
                self.set_status("kb-narrow: not in a KB view");
                return;
            }
        };

        // Query meta-node members from the store.
        let members = if let Some(ref store) = self.kb.store {
            match store.meta_members(&node_id) {
                Ok(m) if !m.is_empty() => m,
                Ok(_) => {
                    self.set_status(format!("'{}' has no meta-members", node_id));
                    return;
                }
                Err(e) => {
                    self.set_status(format!("kb-narrow: {}", e));
                    return;
                }
            }
        } else {
            self.set_status("kb-narrow: no KB store available");
            return;
        };

        // Build completion list from members.
        let items: Vec<(String, String)> = members
            .iter()
            .map(|m| {
                let title = if let Some(q) = self.kb.query_layer() {
                    q.get(&m.member_id).map(|n| n.title)
                } else {
                    self.kb.primary.get(&m.member_id).map(|n| n.title.clone())
                }
                .unwrap_or_else(|| m.member_id.clone());
                (m.member_id.clone(), format!("{} ({})", title, m.role))
            })
            .collect();

        // For simplicity, if there's only one member, open it directly.
        // Otherwise, show first member (full completion UI deferred).
        let member_id = &items[0].0;
        self.kb_open_member_for_editing(&node_id, member_id);
    }

    /// Open a meta-node member for editing in a new buffer.
    ///
    /// Buffer name encodes both IDs: `*kb-narrow:META_ID:MEMBER_ID*`
    pub(super) fn kb_open_member_for_editing(&mut self, meta_id: &str, member_id: &str) {
        let node = if let Some(q) = self.kb.query_layer() {
            q.get(member_id)
        } else {
            self.kb.primary.get(member_id).cloned()
        };
        let node = match node {
            Some(n) => n,
            None => {
                self.set_status(format!("Node '{}' not found", member_id));
                return;
            }
        };

        // Create an edit buffer with the node's body.
        let buf_name = format!("*kb-narrow:{}:{}*", meta_id, member_id);
        let mut buf = crate::Buffer::new();
        buf.name = buf_name;
        buf.insert_text_at(0, &node.body);
        buf.modified = false;

        self.buffers.push(buf);
        let idx = self.buffers.len() - 1;
        self.display_buffer(idx);
        self.set_status(format!(
            "Narrowed to '{}' — :kb-widen to save and return",
            member_id
        ));
    }

    /// Parse meta_id and member_id from a `*kb-narrow:META:MEMBER*` buffer name.
    pub(super) fn parse_narrow_buffer_name(name: &str) -> Option<(String, String)> {
        let inner = name.strip_prefix("*kb-narrow:")?.strip_suffix('*')?;
        let colon = inner.find(':')?;
        let meta_id = &inner[..colon];
        let member_id = &inner[colon + 1..];
        if meta_id.is_empty() || member_id.is_empty() {
            return None;
        }
        Some((meta_id.to_string(), member_id.to_string()))
    }

    /// Save edits from a narrowed meta-node component and widen back.
    pub fn kb_widen_meta(&mut self) {
        let idx = self.active_buffer_idx();
        let buf_name = self.buffers[idx].name.clone();

        // Check if this is a narrowed KB buffer.
        let (meta_id, member_id) = match Self::parse_narrow_buffer_name(&buf_name) {
            Some(ids) => ids,
            None => {
                self.set_status("kb-widen: not in a narrowed KB buffer");
                return;
            }
        };

        // Extract edited content.
        let new_body = self.buffers[idx].text().to_string();

        // Update the node in the primary KB.
        if let Some(node) = self.kb.primary.get_mut(&member_id) {
            node.body.clone_from(&new_body);
        }

        // Update in the CozoDB store if available.
        if let Some(ref store) = self.kb.store {
            if let Some(node) = self.kb.primary.get(&member_id) {
                let _ = store.save_all(&[node]);
            }
            // Recompose the meta-node body.
            if let Ok(composed) = store.compose_meta_body(&meta_id) {
                if let Some(meta_node) = self.kb.primary.get_mut(&meta_id) {
                    meta_node.body = composed;
                }
            }
        }

        // Close the narrow buffer and return.
        self.buffers.remove(idx);
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx >= idx {
                win.buffer_idx = win.buffer_idx.saturating_sub(1);
            }
        }
        let ret = idx.min(self.buffers.len().saturating_sub(1));
        self.display_buffer(ret);
        self.set_status(format!("Widened from '{}', changes saved", member_id));
    }
}
