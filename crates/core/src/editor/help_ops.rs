//! KB-buffer operations — commands that manipulate *Help*/*KB* buffers
//! and their underlying KB navigation state.
//!
//! The dispatch layer calls these as part of `dispatch_builtin`; the AI
//! agent calls the KB directly via its `kb_*` tools (no need for these
//! view-layer helpers).

use crate::buffer::BufferKind;
use crate::kb_view::KbLinkSpan;

use super::Editor;

/// Returns true if the node ID belongs to the built-in MAE manual
/// (commands, concepts, lessons, scheme API, options, keys, modules, tutorials).
/// User-created nodes (dailies, federated, personal) return false.
pub fn is_builtin_node(id: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "cmd:",
        "concept:",
        "lesson:",
        "scheme:",
        "option:",
        "key:",
        "module:",
        "tutorial:",
    ];
    id == "index" || PREFIXES.iter().any(|p| id.starts_with(p))
}

fn node_kind_label(kind: mae_kb::NodeKind) -> &'static str {
    kind.as_str()
}

/// Render a KB node into plain text and extract link byte ranges.
/// Returns `(rendered_text, link_spans)`.
///
/// Uses the `KbQueryLayer` for node lookup, typed links, and title resolution.
/// The query layer already returns typed `Link` with `rel_type`, so no separate
/// store parameter is needed.
fn render_kb_node_for_query(
    query: &dyn mae_kb::KbQueryLayer,
    node_id: &str,
) -> (String, Vec<KbLinkSpan>) {
    let mut out = String::new();
    let mut links: Vec<KbLinkSpan> = Vec::new();

    let Some(node) = query.get(node_id) else {
        out.push_str(&format!("(no such KB node: {})\n", node_id));
        return (out, links);
    };

    // Header — * prefix gives h1 scale in GUI heading renderer
    out.push_str(&format!("* {}", node.title));
    out.push('\n');
    let content_label = if is_builtin_node(node_id) {
        "MAE Manual"
    } else {
        "Knowledge Base"
    };
    out.push_str(&format!(
        "{} · {} · {}\n",
        content_label,
        node_kind_label(node.kind),
        node.id
    ));
    if !node.tags.is_empty() {
        out.push_str(&format!("tags: {}\n", node.tags.join(", ")));
    }
    out.push('\n');

    // Body — parse [[target|display]] link markers, strip property drawers
    let mut in_drawer = false;
    let header_lines = out.lines().count();
    for body_line in node.body.lines() {
        let trimmed = body_line.trim();
        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") || trimmed.eq_ignore_ascii_case(":LOGBOOK:")
        {
            in_drawer = true;
            continue;
        }
        if in_drawer {
            if trimmed.eq_ignore_ascii_case(":END:") {
                in_drawer = false;
            }
            continue;
        }
        // Strip #+keyword lines near top (already in header metadata)
        if trimmed.starts_with("#+") && out.lines().count() < header_lines + 4 {
            continue;
        }
        render_body_line(body_line, &mut out, &mut links);
        out.push('\n');
    }

    // Neighborhood — query layer returns typed links directly
    let outgoing = query.links_from(node_id);
    let incoming = query.links_to(node_id);

    if !outgoing.is_empty() || !incoming.is_empty() {
        out.push('\n');
        out.push_str("** Neighborhood\n");
    }
    if !outgoing.is_empty() {
        out.push_str("Outgoing:\n");
        for link in &outgoing {
            let title_text = query
                .get(&link.dst)
                .map(|n| n.title)
                .unwrap_or_else(|| "(missing)".to_string());
            out.push_str("  ");
            if link.rel_type != "references" {
                out.push_str(&link.rel_type);
                out.push_str(" → ");
            } else {
                out.push_str("→ ");
            }
            let link_start = out.len();
            out.push_str(&link.dst);
            let link_end = out.len();
            links.push(KbLinkSpan {
                byte_start: link_start,
                byte_end: link_end,
                target: link.dst.clone(),
            });
            out.push_str(&format!("  {}\n", title_text));
        }
    }
    if !incoming.is_empty() {
        out.push_str(&format!("Backlinks ({}):\n", incoming.len()));
        for link in &incoming {
            let title_text = query
                .get(&link.src)
                .map(|n| n.title)
                .unwrap_or_else(|| "(missing)".to_string());
            out.push_str("  ");
            if link.rel_type != "references" {
                out.push_str(&link.rel_type);
                out.push_str(" ← ");
            } else {
                out.push_str("← ");
            }
            let link_start = out.len();
            out.push_str(&link.src);
            let link_end = out.len();
            links.push(KbLinkSpan {
                byte_start: link_start,
                byte_end: link_end,
                target: link.src.clone(),
            });
            out.push_str(&format!("  {}\n", title_text));
        }
    }

    out.push('\n');
    out.push_str(
        "Tab: fold · n/p: links · Enter: follow · e: edit · C-o/C-i: back/fwd · q: close\n",
    );

    (out, links)
}

/// Render a KB node using the in-memory `KnowledgeBase` as fallback when
/// no query layer is available.
fn render_kb_node_with_store(
    kb: &mae_kb::KnowledgeBase,
    node_id: &str,
    resolve_title: impl Fn(&str) -> Option<String>,
    store: Option<&dyn mae_kb::KbStore>,
) -> (String, Vec<KbLinkSpan>) {
    let mut out = String::new();
    let mut links: Vec<KbLinkSpan> = Vec::new();

    let Some(node) = kb.get(node_id) else {
        out.push_str(&format!("(no such KB node: {})\n", node_id));
        return (out, links);
    };

    out.push_str(&format!("* {}", node.title));
    out.push('\n');
    let content_label = if is_builtin_node(node_id) {
        "MAE Manual"
    } else {
        "Knowledge Base"
    };
    out.push_str(&format!(
        "{} · {} · {}\n",
        content_label,
        node_kind_label(node.kind),
        node.id
    ));
    if !node.tags.is_empty() {
        out.push_str(&format!("tags: {}\n", node.tags.join(", ")));
    }
    out.push('\n');

    let mut in_drawer = false;
    let header_lines = out.lines().count();
    for body_line in node.body.lines() {
        let trimmed = body_line.trim();
        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") || trimmed.eq_ignore_ascii_case(":LOGBOOK:")
        {
            in_drawer = true;
            continue;
        }
        if in_drawer {
            if trimmed.eq_ignore_ascii_case(":END:") {
                in_drawer = false;
            }
            continue;
        }
        if trimmed.starts_with("#+") && out.lines().count() < header_lines + 4 {
            continue;
        }
        render_body_line(body_line, &mut out, &mut links);
        out.push('\n');
    }

    let (outgoing_typed, incoming_typed) = if let Some(st) = store {
        let out_links = st.links_from(node_id).unwrap_or_default();
        let in_links = st.links_to(node_id).unwrap_or_default();
        (Some(out_links), Some(in_links))
    } else {
        (None, None)
    };

    let outgoing_ids = kb.links_from(node_id);
    let incoming_ids = kb.links_to(node_id);

    if !outgoing_ids.is_empty() || !incoming_ids.is_empty() {
        out.push('\n');
        out.push_str("** Neighborhood\n");
    }
    if !outgoing_ids.is_empty() {
        out.push_str("Outgoing:\n");
        for target in &outgoing_ids {
            let title_text = resolve_title(target).unwrap_or_else(|| "(missing)".to_string());
            let rel_label = outgoing_typed.as_ref().and_then(|typed| {
                typed.iter().find(|l| l.dst == *target).and_then(|l| {
                    if l.rel_type != "references" {
                        Some(l.rel_type.as_str())
                    } else {
                        None
                    }
                })
            });
            out.push_str("  ");
            if let Some(rel) = rel_label {
                out.push_str(rel);
                out.push_str(" → ");
            } else {
                out.push_str("→ ");
            }
            let link_start = out.len();
            out.push_str(target);
            let link_end = out.len();
            links.push(KbLinkSpan {
                byte_start: link_start,
                byte_end: link_end,
                target: target.clone(),
            });
            out.push_str(&format!("  {}\n", title_text));
        }
    }
    if !incoming_ids.is_empty() {
        out.push_str(&format!("Backlinks ({}):\n", incoming_ids.len()));
        for src in &incoming_ids {
            let title_text = resolve_title(src).unwrap_or_else(|| "(missing)".to_string());
            let rel_label = incoming_typed.as_ref().and_then(|typed| {
                typed.iter().find(|l| l.src == *src).and_then(|l| {
                    if l.rel_type != "references" {
                        Some(l.rel_type.as_str())
                    } else {
                        None
                    }
                })
            });
            out.push_str("  ");
            if let Some(rel) = rel_label {
                out.push_str(rel);
                out.push_str(" ← ");
            } else {
                out.push_str("← ");
            }
            let link_start = out.len();
            out.push_str(src);
            let link_end = out.len();
            links.push(KbLinkSpan {
                byte_start: link_start,
                byte_end: link_end,
                target: src.clone(),
            });
            out.push_str(&format!("  {}\n", title_text));
        }
    }

    out.push('\n');
    out.push_str(
        "Tab: fold · n/p: links · Enter: follow · e: edit · C-o/C-i: back/fwd · q: close\n",
    );

    (out, links)
}

/// Render a single body line, stripping `[[target|display]]` markers and
/// recording link spans.
fn render_body_line(line: &str, out: &mut String, links: &mut Vec<KbLinkSpan>) {
    let bytes = line.as_bytes();
    let mut cursor = 0usize;
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(end_rel) = line[i + 2..].find("]]") {
                let inner = &line[i + 2..i + 2 + end_rel];
                let (target, display) = match inner.find('|') {
                    Some(bar) => (inner[..bar].trim(), inner[bar + 1..].trim()),
                    None => {
                        let t = inner.trim();
                        (t, t)
                    }
                };
                if !target.is_empty() {
                    // Emit text before the link
                    out.push_str(&line[cursor..i]);
                    // Emit link display text
                    let link_start = out.len();
                    out.push_str(display);
                    let link_end = out.len();
                    links.push(KbLinkSpan {
                        byte_start: link_start,
                        byte_end: link_end,
                        target: target.to_string(),
                    });
                    cursor = i + 2 + end_rel + 2;
                    i = cursor;
                    continue;
                }
            }
        }
        i += 1;
    }
    // Remainder
    out.push_str(&line[cursor..]);
}

impl Editor {
    /// Generate live help text for a command, querying current keymaps and hooks.
    pub fn describe_command_live(&self, cmd_name: &str) -> Option<String> {
        let cmd = self.commands.get(cmd_name)?;
        let mut out = String::new();
        out.push_str(&format!("* {}\n", cmd_name));
        out.push_str(&cmd.doc);
        out.push('\n');

        let category = crate::kb_seed::infer_category(cmd_name);
        out.push_str(&format!("\n**Category:** {}\n", category));

        let source = match &cmd.source {
            crate::commands::CommandSource::Builtin => "Builtin".to_string(),
            crate::commands::CommandSource::Scheme(f) => format!("Scheme (`{}`)", f),
            crate::commands::CommandSource::Autoload { feature } => {
                format!("Autoload (feature `{}`)", feature)
            }
        };
        out.push_str(&format!("**Source:** {}\n", source));

        let bindings = crate::kb_seed::collect_keybindings_for(&self.keymaps, cmd_name);
        if !bindings.is_empty() {
            out.push_str("\n**Keybindings:**\n");
            for (mode, key) in &bindings {
                out.push_str(&format!("  {}: `{}`\n", mode, key));
            }
        }

        let hook_names = self.hooks.hooks_containing(cmd_name);
        if !hook_names.is_empty() {
            out.push_str(&format!("\n**Hooks:** {}\n", hook_names.join(", ")));
        } else {
            out.push_str("\n**Hooks:** (none)\n");
        }

        out.push_str(&format!(
            "\nSee also: [[cmd:move-right]], [[category:{}]]\n",
            category
        ));

        Some(out)
    }

    /// Open the *Help* buffer on the given KB node, creating it if it
    /// doesn't exist. Falls back to the `index` node if the requested id
    /// isn't found.
    /// Check if a node ID exists in the local KB or any federated instance.
    fn kb_contains_any(&self, id: &str) -> bool {
        if let Some(q) = self.kb.query_layer() {
            return q.contains(id);
        }
        if self.kb.primary.contains(id) {
            return true;
        }
        self.kb.instances.values().any(|kb| kb.contains(id))
    }

    /// Resolve a node title across local + federated KBs.
    fn kb_resolve_title(&self, id: &str) -> Option<String> {
        if let Some(q) = self.kb.query_layer() {
            return q.get(id).map(|n| n.title);
        }
        if let Some(n) = self.kb.primary.get(id) {
            return Some(n.title.clone());
        }
        for kb in self.kb.instances.values() {
            if let Some(n) = kb.get(id) {
                return Some(n.title.clone());
            }
        }
        None
    }

    /// Get the KnowledgeBase that contains a given node ID (local first, then federated).
    fn kb_for_node(&self, id: &str) -> Option<&mae_kb::KnowledgeBase> {
        // Use query_layer for the existence check when available, but we still need
        // to return a &KnowledgeBase reference so we do the structural search regardless.
        if let Some(q) = self.kb.query_layer() {
            if q.contains(id) {
                // Node exists somewhere; prefer primary, fall back to instances.
                if self.kb.primary.contains(id) {
                    return Some(&self.kb.primary);
                }
                return self.kb.instances.values().find(|kb| kb.contains(id));
            }
            return None;
        }
        if self.kb.primary.contains(id) {
            return Some(&self.kb.primary);
        }
        self.kb.instances.values().find(|kb| kb.contains(id))
    }

    pub fn open_help_at(&mut self, node_id: &str) {
        let target = if self.kb_contains_any(node_id) {
            node_id.to_string()
        } else {
            // Try namespace prefix expansion: "buffer" → "concept:buffer", "save" → "cmd:save"
            let mut found = None;
            let ns_prefixes = if let Some(q) = self.kb.query_layer() {
                q.namespace_prefixes()
            } else {
                self.kb.primary.namespace_prefixes()
            };
            for prefix in ns_prefixes {
                let expanded = format!("{}{}", prefix, node_id);
                if self.kb_contains_any(&expanded) {
                    found = Some(expanded);
                    break;
                }
            }
            // Fall back to fuzzy search top result (local + federated).
            if found.is_none() {
                let results = self.kb_federated_search(node_id);
                if let Some((_, node)) = results.into_iter().next() {
                    if node.id != "index" {
                        found = Some(node.id.clone());
                    }
                }
            }
            match found {
                Some(resolved) => resolved,
                None => {
                    self.set_status(format!("No help node: {}  — showing index", node_id));
                    "index".to_string()
                }
            }
        };
        // Record access for activity tracking (UserOrg notes only).
        self.kb_record_access(&target);
        // Record the visit for recency ordering (KbSort::Recency).
        self.kb.record_visit(&target);

        let prev_idx = self.active_buffer_idx();
        let idx = self.ensure_kb_buffer_idx(&target);
        if idx != prev_idx {
            self.vi.alternate_buffer_idx = Some(prev_idx);
        }
        self.kb_populate_buffer(idx);
        self.display_buffer(idx);
    }

    /// Render the current KB node into the KB buffer's rope and store
    /// link spans. Called on every navigation (open, follow link, back/forward).
    pub fn kb_populate_buffer(&mut self, buf_idx: usize) {
        let node_id = match self.buffers[buf_idx].kb_view() {
            Some(v) => v.current.clone(),
            None => return,
        };
        let (text, link_spans) = if node_id.starts_with("cmd:") {
            let cmd_name = node_id.strip_prefix("cmd:").unwrap();
            if let Some(live_text) = self.describe_command_live(cmd_name) {
                // Re-render links from the live text
                let mut out = String::new();
                let mut links = Vec::new();
                // Add header info from KB node if it exists
                let header_node = if let Some(q) = self.kb.query_layer() {
                    q.get(&node_id)
                } else {
                    self.kb.primary.get(&node_id).cloned()
                };
                if let Some(node) = header_node {
                    out.push_str(&format!("* {}", node.title));
                    out.push('\n');
                    out.push_str(&format!("{} · {}\n", node_kind_label(node.kind), node.id));
                    if !node.tags.is_empty() {
                        out.push_str(&format!("tags: {}\n", node.tags.join(", ")));
                    }
                    out.push('\n');
                }
                // Parse the live text for links
                for body_line in live_text.lines() {
                    render_body_line(body_line, &mut out, &mut links);
                    out.push('\n');
                }
                // Add neighborhood from KB (federation-aware, typed links)
                let outgoing_links = if let Some(q) = self.kb.query_layer() {
                    q.links_from(&node_id)
                } else {
                    self.kb
                        .primary
                        .links_from(&node_id)
                        .into_iter()
                        .map(|dst| mae_kb::store::Link {
                            src: node_id.clone(),
                            dst,
                            rel_type: "references".into(),
                            display: None,
                            weight: 1.0,
                            confidence: 1.0,
                        })
                        .collect()
                };
                let incoming_links = if let Some(q) = self.kb.query_layer() {
                    q.links_to(&node_id)
                } else {
                    self.kb
                        .primary
                        .links_to(&node_id)
                        .into_iter()
                        .map(|src| mae_kb::store::Link {
                            src,
                            dst: node_id.clone(),
                            rel_type: "references".into(),
                            display: None,
                            weight: 1.0,
                            confidence: 1.0,
                        })
                        .collect()
                };
                if !outgoing_links.is_empty() || !incoming_links.is_empty() {
                    out.push('\n');
                    out.push_str("** Neighborhood\n");
                }
                if !outgoing_links.is_empty() {
                    out.push_str("Outgoing:\n");
                    for link in &outgoing_links {
                        let title_text = self
                            .kb_resolve_title(&link.dst)
                            .unwrap_or_else(|| "(missing)".to_string());
                        out.push_str("  ");
                        if link.rel_type != "references" {
                            out.push_str(&link.rel_type);
                            out.push_str(" → ");
                        } else {
                            out.push_str("→ ");
                        }
                        let link_start = out.len();
                        out.push_str(&link.dst);
                        let link_end = out.len();
                        links.push(KbLinkSpan {
                            byte_start: link_start,
                            byte_end: link_end,
                            target: link.dst.clone(),
                        });
                        out.push_str(&format!("  {}\n", title_text));
                    }
                }
                if !incoming_links.is_empty() {
                    out.push_str(&format!("Backlinks ({}):\n", incoming_links.len()));
                    for link in &incoming_links {
                        let title_text = self
                            .kb_resolve_title(&link.src)
                            .unwrap_or_else(|| "(missing)".to_string());
                        out.push_str("  ");
                        if link.rel_type != "references" {
                            out.push_str(&link.rel_type);
                            out.push_str(" ← ");
                        } else {
                            out.push_str("← ");
                        }
                        let link_start = out.len();
                        out.push_str(&link.src);
                        let link_end = out.len();
                        links.push(KbLinkSpan {
                            byte_start: link_start,
                            byte_end: link_end,
                            target: link.src.clone(),
                        });
                        out.push_str(&format!("  {}\n", title_text));
                    }
                }
                out.push('\n');
                out.push_str(
                    "Tab: fold · n/p: links · Enter: follow · e: edit · C-o/C-i: back/fwd · q: close\n",
                );
                (out, links)
            } else if let Some(q) = self.kb.query_layer() {
                render_kb_node_for_query(q, &node_id)
            } else {
                let kb = self.kb_for_node(&node_id).unwrap_or(&self.kb.primary);
                let local = &self.kb.primary;
                let federated = &self.kb.instances;
                let store_ref = self.kb.store.as_deref();
                render_kb_node_with_store(
                    kb,
                    &node_id,
                    |id| {
                        local.get(id).map(|n| n.title.clone()).or_else(|| {
                            federated
                                .values()
                                .find_map(|fkb| fkb.get(id).map(|n| n.title.clone()))
                        })
                    },
                    store_ref,
                )
            }
        } else if let Some(q) = self.kb.query_layer() {
            render_kb_node_for_query(q, &node_id)
        } else {
            let kb = self.kb_for_node(&node_id).unwrap_or(&self.kb.primary);
            let local = &self.kb.primary;
            let federated = &self.kb.instances;
            let store_ref = self.kb.store.as_deref();
            render_kb_node_with_store(
                kb,
                &node_id,
                |id| {
                    local.get(id).map(|n| n.title.clone()).or_else(|| {
                        federated
                            .values()
                            .find_map(|fkb| fkb.get(id).map(|n| n.title.clone()))
                    })
                },
                store_ref,
            )
        };
        // Temporarily allow writing to the read-only buffer.
        self.buffers[buf_idx].read_only = false;
        self.buffers[buf_idx].replace_contents(&text);
        self.buffers[buf_idx].read_only = true;
        // Detect broken links for visual feedback (before borrowing buffers mutably).
        let mut broken = std::collections::HashSet::new();
        for (i, link) in link_spans.iter().enumerate() {
            if !self.kb_contains_any(&link.target) {
                broken.insert(i);
            }
        }
        if let Some(view) = self.buffers[buf_idx].kb_view_mut() {
            view.rendered_links = link_spans;
            view.broken_links = broken;
        }
    }

    /// Navigable link targets from the rendered KB buffer, in document
    /// order. Backed by `KbView.rendered_links` (populated by
    /// `kb_populate_buffer`). This replaces the old KB-neighbor lookup.
    pub fn kb_navigable_links(&self) -> Vec<String> {
        match self.kb_view() {
            Some(view) => view
                .rendered_links
                .iter()
                .map(|l| l.target.clone())
                .collect(),
            None => Vec::new(),
        }
    }

    /// Follow the currently-focused link in the *Help* buffer.
    /// If no link is focused but the cursor is on a link, follow that one.
    pub fn help_follow_link(&mut self) {
        // If no link is explicitly focused, check if cursor is on a link.
        let cursor_byte = self.help_cursor_byte_offset();
        if let Some(view) = self.kb_view_mut() {
            if view.focused_link.is_none() {
                // Find link under cursor.
                if let Some(idx) = view
                    .rendered_links
                    .iter()
                    .position(|l| cursor_byte >= l.byte_start && cursor_byte < l.byte_end)
                {
                    view.focused_link = Some(idx);
                }
            }
        }
        let (target, buf_idx, fragment) = {
            let Some(view) = self.kb_view() else {
                self.set_status("Not in a help buffer");
                return;
            };
            let Some(idx) = view.focused_link else {
                self.set_status("No link under cursor (Tab to move between links)");
                return;
            };
            let Some(link) = view.rendered_links.get(idx) else {
                return;
            };
            let mut target = link.target.clone();
            // Split off fragment (e.g., "concept:buffer#architecture")
            let fragment = if let Some(hash_pos) = target.find('#') {
                let frag = target[hash_pos + 1..].to_string();
                target = target[..hash_pos].to_string();
                Some(frag)
            } else {
                None
            };
            if !self.kb_contains_any(&target) {
                // Attempt fuzzy resolution via federated search
                let results = self.kb_federated_search(&target);
                if results.len() == 1 {
                    let resolved_id = results[0].1.id.clone();
                    self.set_status(format!("Resolved: {} → {}", target, resolved_id));
                    target = resolved_id;
                } else {
                    self.set_status(format!(
                        "Link not found: '{}' — try :help {}",
                        target, target
                    ));
                    return;
                }
            }
            let Some(buf_idx) = self.buffers.iter().position(|b| b.kind == BufferKind::Kb) else {
                return;
            };
            (target, buf_idx, fragment)
        };
        if let Some(view) = self.kb_view_mut() {
            view.navigate_to(target);
        }
        self.kb_populate_buffer(buf_idx);
        // Handle fragment navigation: scroll to heading or block index
        let frag_row = fragment.and_then(|frag| {
            let rope = self.buffers[buf_idx].rope();
            if let Ok(idx) = frag.parse::<usize>() {
                // Numeric fragment: jump to paragraph block N
                // Count blank-line-separated blocks
                let mut block = 0;
                for (line_idx, line) in rope.lines().enumerate() {
                    let text: String = line.chars().collect();
                    if text.trim().is_empty() {
                        continue;
                    }
                    if block == idx {
                        return Some(line_idx);
                    }
                    // A block boundary is a non-empty line after a blank line
                    if line_idx > 0 {
                        let prev: String = rope.line(line_idx - 1).chars().collect();
                        if prev.trim().is_empty() {
                            block += 1;
                            if block == idx {
                                return Some(line_idx);
                            }
                        }
                    }
                }
                None
            } else {
                // Named fragment: search for heading matching the slug
                let slug_lower = frag.to_lowercase().replace(['-', '_'], " ");
                for (line_idx, line) in rope.lines().enumerate() {
                    let text: String = line.chars().collect();
                    let trimmed = text.trim_start();
                    if trimmed.starts_with('#') || trimmed.starts_with('*') {
                        // Extract heading text (strip # or * prefix)
                        let heading = trimmed.trim_start_matches(['#', '*']).trim();
                        let heading_slug = heading.to_lowercase();
                        if heading_slug.contains(&slug_lower) {
                            return Some(line_idx);
                        }
                    }
                }
                None
            }
        });
        if let Some(row) = frag_row {
            self.window_mgr.focused_window_mut().cursor_row = row;
        } else {
            self.window_mgr.focused_window_mut().cursor_row = 0;
        }
        self.window_mgr.focused_window_mut().cursor_col = 0;
    }

    pub fn help_back(&mut self) {
        let went_back = if let Some(view) = self.kb_view_mut() {
            view.go_back()
        } else {
            false
        };
        if went_back {
            if let Some(buf_idx) = self.buffers.iter().position(|b| b.kind == BufferKind::Kb) {
                self.kb_populate_buffer(buf_idx);
                self.window_mgr.focused_window_mut().cursor_row = 0;
                self.window_mgr.focused_window_mut().cursor_col = 0;
            }
        } else {
            self.set_status("No previous help page");
            self.ring_bell();
        }
    }

    pub fn help_forward(&mut self) {
        let went_fwd = if let Some(view) = self.kb_view_mut() {
            view.go_forward()
        } else {
            false
        };
        if went_fwd {
            if let Some(buf_idx) = self.buffers.iter().position(|b| b.kind == BufferKind::Kb) {
                self.kb_populate_buffer(buf_idx);
                self.window_mgr.focused_window_mut().cursor_row = 0;
                self.window_mgr.focused_window_mut().cursor_col = 0;
            }
        } else {
            self.set_status("No forward help page");
            self.ring_bell();
        }
    }

    pub fn help_next_link(&mut self) {
        let cursor_byte = self.help_cursor_byte_offset();
        if let Some(view) = self.kb_view_mut() {
            view.focus_next_link(cursor_byte);
        }
        self.help_move_cursor_to_focused_link();
    }

    pub fn help_prev_link(&mut self) {
        let cursor_byte = self.help_cursor_byte_offset();
        if let Some(view) = self.kb_view_mut() {
            view.focus_prev_link(cursor_byte);
        }
        self.help_move_cursor_to_focused_link();
    }

    /// Move the cursor to the start of the currently focused link so the
    /// viewport scrolls to show it and the user sees where they landed.
    fn help_move_cursor_to_focused_link(&mut self) {
        let byte_start = match self.kb_view() {
            Some(view) => match view.focused_link {
                Some(idx) => match view.rendered_links.get(idx) {
                    Some(link) => link.byte_start,
                    None => return,
                },
                None => return,
            },
            None => return,
        };
        let Some(buf_idx) = self.buffers.iter().position(|b| b.kind == BufferKind::Kb) else {
            return;
        };
        let rope = self.buffers[buf_idx].rope();
        let row = rope.byte_to_line(byte_start);
        let line_byte_start = rope.line_to_byte(row);
        let col = byte_start - line_byte_start;
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = row;
        win.cursor_col = col;
    }

    /// Compute the byte offset in the KB buffer's rope corresponding to the cursor position.
    fn help_cursor_byte_offset(&self) -> usize {
        let buf_idx = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Kb)
            .unwrap_or_else(|| self.active_buffer_idx());
        let buf = &self.buffers[buf_idx];
        let win = self.window_mgr.focused_window();
        let rope = buf.rope();
        let row = win.cursor_row.min(rope.len_lines().saturating_sub(1));
        let line_start = rope.line_to_byte(row);
        let line_len = rope.line(row).len_bytes();
        let col_bytes = win.cursor_col.min(line_len);
        line_start + col_bytes
    }

    /// Close the *Help* buffer if one exists, switching to the alternate
    /// buffer (or scratch). Saves the view state for `help-reopen`.
    pub fn help_close(&mut self) {
        let help_idx = self.buffers.iter().position(|b| b.kind == BufferKind::Kb);
        let Some(help_idx) = help_idx else {
            return;
        };
        // Save state for reopen.
        self.last_kb_state = self.buffers[help_idx].kb_view().cloned();
        // Pick a sensible destination: alternate if set (and not the
        // KB buffer itself), otherwise the first non-KB buffer.
        let dest_idx = self
            .vi
            .alternate_buffer_idx
            .filter(|&i| i != help_idx && i < self.buffers.len())
            .or_else(|| self.buffers.iter().position(|b| b.kind != BufferKind::Kb))
            .unwrap_or(0);
        // Retarget any window focused on help before we remove it.
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx == help_idx {
                win.buffer_idx = dest_idx;
                win.cursor_row = 0;
                win.cursor_col = 0;
            }
        }
        self.buffers.remove(help_idx);
        self.notify_buffer_removed(help_idx);
        // Fix indices that were above the removed buffer.
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx > help_idx {
                win.buffer_idx -= 1;
            }
        }
    }

    /// Jump from the current KB buffer node to its source `.org` file.
    /// Works for federated nodes that have `source_file` stamped during ingest.
    pub fn help_edit_source(&mut self) {
        // Get current help node ID
        let node_id = match self.kb_view() {
            Some(view) => view.current.clone(),
            None => {
                self.set_status("Not in a help buffer");
                return;
            }
        };

        // Look up the node (local first, then federated) and get source_file
        let source_file = self
            .kb
            .primary
            .get(&node_id)
            .or_else(|| self.kb.instances.values().find_map(|kb| kb.get(&node_id)))
            .and_then(|n| n.source_file.clone());

        match source_file {
            Some(path) => {
                let path_str = path.display().to_string();
                self.open_file(&path_str);
            }
            None => {
                self.set_status(format!("No source file for '{}'", node_id));
            }
        }
    }

    /// Return to the rendered KB view from source editing.
    /// If a KB buffer exists, switch to it. Otherwise, reopen the last one.
    pub fn help_return_to_view(&mut self) {
        if let Some(idx) = self.buffers.iter().position(|b| b.kind == BufferKind::Kb) {
            // Refresh the help content before showing it
            self.kb_populate_buffer(idx);
            // Replace focused window directly (not via display_policy which may split)
            let win = self.window_mgr.focused_window_mut();
            win.buffer_idx = idx;
            win.cursor_row = 0;
            win.cursor_col = 0;
            self.sync_mode_to_buffer();
            self.mark_full_redraw();
        } else if self.last_kb_state.is_some() {
            self.help_reopen();
        } else if let Some(id) = self.kb_node_id_for_active_buffer() {
            let prev_idx = self.active_buffer_idx();
            let idx = self.ensure_kb_buffer_idx(&id);
            self.kb_populate_buffer(idx);
            if idx != prev_idx {
                self.vi.alternate_buffer_idx = Some(prev_idx);
            }
            let win = self.window_mgr.focused_window_mut();
            win.buffer_idx = idx;
            win.cursor_row = 0;
            win.cursor_col = 0;
            self.sync_mode_to_buffer();
            self.mark_full_redraw();
        } else {
            self.set_status("No KB view to return to");
        }
    }

    /// Infer a KB node ID from the currently active buffer's file path.
    /// Matches daily files (`YYYY-MM-DD.org` → `daily:YYYY-MM-DD`) and
    /// KB nodes whose `source_file` metadata matches the buffer path.
    pub(crate) fn kb_node_id_for_active_buffer(&self) -> Option<String> {
        let buf = self.active_buffer();
        let path = buf.file_path()?;
        let stem = path.file_stem()?.to_str()?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        // Daily pattern: YYYY-MM-DD.org
        if ext == "org" && stem.len() == 10 && stem.chars().nth(4) == Some('-') {
            let daily_id = format!("daily:{}", stem);
            if self.kb_contains_any(&daily_id) {
                return Some(daily_id);
            }
        }

        // Search KB nodes by source_file metadata
        let primary_ids = if let Some(q) = self.kb.query_layer() {
            q.list_ids(None)
        } else {
            self.kb.primary.list_ids(None)
        };
        for id in primary_ids {
            let node = if let Some(q) = self.kb.query_layer() {
                q.get(&id)
            } else {
                self.kb.primary.get(&id).cloned()
            };
            if let Some(node) = node {
                if let Some(ref sf) = node.source_file {
                    if sf == path {
                        return Some(id);
                    }
                }
            }
        }
        for kb in self.kb.instances.values() {
            for id in kb.list_ids(None) {
                if let Some(node) = kb.get(&id) {
                    if let Some(ref sf) = node.source_file {
                        if sf == path {
                            return Some(id);
                        }
                    }
                }
            }
        }

        None
    }

    /// Re-render the KB buffer if it exists and the underlying KB node has changed.
    /// Called after save, focus-in, or KB reimport.
    pub fn refresh_help_if_stale(&mut self) {
        let help_idx = match self.buffers.iter().position(|b| b.kind == BufferKind::Kb) {
            Some(idx) => idx,
            None => return,
        };
        // Always repopulate — the KB may have changed.
        // kb_populate_buffer is cheap (string formatting, no I/O).
        self.kb_populate_buffer(help_idx);
    }

    // --- Help buffer heading folding (Fix 4) ---

    /// Heading level for a KB buffer line (language-agnostic: both `*` and `#`).
    fn help_heading_level_at(&self, buf_idx: usize, row: usize) -> u8 {
        let rope = self.buffers[buf_idx].rope();
        if row >= rope.len_lines() {
            return 0;
        }
        let line = rope.line(row);
        let chars: Vec<char> = line.chars().take(10).collect();
        crate::heading::heading_level_from_chars(&chars)
    }

    /// Find the end of a heading's subtree (next heading at same-or-shallower level).
    fn help_subtree_end(&self, buf_idx: usize, row: usize, level: u8) -> usize {
        let total = self.buffers[buf_idx].line_count();
        let mut end = row + 1;
        while end < total {
            let l = self.help_heading_level_at(buf_idx, end);
            if l > 0 && l <= level {
                break;
            }
            end += 1;
        }
        end
    }

    /// Tab on a heading → fold/unfold subtree. Not on heading → next link.
    pub fn help_heading_cycle(&mut self) {
        let buf_idx = match self.buffers.iter().position(|b| b.kind == BufferKind::Kb) {
            Some(i) => i,
            None => return,
        };
        let row = self.window_mgr.focused_window().cursor_row;
        let level = self.help_heading_level_at(buf_idx, row);
        if level == 0 {
            // Not on a heading — fall through to next link
            self.help_next_link();
            return;
        }
        let end = self.help_subtree_end(buf_idx, row, level);
        if row >= end.saturating_sub(1) {
            return; // single-line heading
        }
        let fold_ranges = vec![(row, end)];
        self.buffers[buf_idx].toggle_fold_at(row, &fold_ranges);
    }

    /// Global visibility cycle: OVERVIEW → CONTENTS → SHOW ALL.
    pub fn help_heading_global_cycle(&mut self) {
        let buf_idx = match self.buffers.iter().position(|b| b.kind == BufferKind::Kb) {
            Some(i) => i,
            None => return,
        };
        let total = self.buffers[buf_idx].line_count();
        // Collect all headings
        let mut headings: Vec<(usize, u8)> = Vec::new();
        for row in 0..total {
            let level = self.help_heading_level_at(buf_idx, row);
            if level > 0 {
                headings.push((row, level));
            }
        }
        if headings.is_empty() {
            return;
        }
        let has_folds = !self.buffers[buf_idx].folded_ranges.is_empty();
        if !has_folds {
            // SHOW ALL → OVERVIEW: fold all headings
            self.buffers[buf_idx].read_only = false;
            for &(row, level) in &headings {
                let end = self.help_subtree_end(buf_idx, row, level);
                if end > row + 1 {
                    self.buffers[buf_idx].folded_ranges.push((row, end));
                }
            }
            self.buffers[buf_idx].read_only = true;
            self.set_status("Overview");
        } else {
            // Has folds → SHOW ALL: clear all
            self.buffers[buf_idx].folded_ranges.clear();
            self.set_status("Show All");
        }
    }

    /// Close all folds in KB buffer (zM).
    pub fn help_close_all_folds(&mut self) {
        let buf_idx = match self.buffers.iter().position(|b| b.kind == BufferKind::Kb) {
            Some(i) => i,
            None => return,
        };
        let total = self.buffers[buf_idx].line_count();
        self.buffers[buf_idx].folded_ranges.clear();
        for row in 0..total {
            let level = self.help_heading_level_at(buf_idx, row);
            if level > 0 {
                let end = self.help_subtree_end(buf_idx, row, level);
                if end > row + 1 {
                    self.buffers[buf_idx].folded_ranges.push((row, end));
                }
            }
        }
        self.set_status("All folds closed");
    }

    /// Open all folds in KB buffer (zR).
    pub fn help_open_all_folds(&mut self) {
        let buf_idx = match self.buffers.iter().position(|b| b.kind == BufferKind::Kb) {
            Some(i) => i,
            None => return,
        };
        self.buffers[buf_idx].folded_ranges.clear();
        self.set_status("All folds opened");
    }

    /// Reopen the last-closed KB buffer at exactly the node and
    /// navigation state where the user left off.
    pub fn help_reopen(&mut self) {
        let Some(saved) = self.last_kb_state.take() else {
            self.set_status("No previous help session");
            return;
        };
        let node_id = saved.current.clone();
        let prev_idx = self.active_buffer_idx();
        let idx = self.ensure_kb_buffer_idx(&node_id);
        // Restore full navigation state (back/forward stacks, focused link).
        self.buffers[idx].view = crate::buffer_view::BufferView::Kb(Box::new(saved));
        self.kb_populate_buffer(idx);
        if idx != prev_idx {
            self.vi.alternate_buffer_idx = Some(prev_idx);
        }
        // Replace focused window directly (not via display_policy which may split).
        let win = self.window_mgr.focused_window_mut();
        win.buffer_idx = idx;
        win.cursor_row = 0;
        win.cursor_col = 0;
        self.sync_mode_to_buffer();
        self.mark_full_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_help_at_creates_buffer() {
        let mut e = Editor::new();
        e.open_help_at("index");
        assert_eq!(e.active_buffer().kind, BufferKind::Kb);
        assert_eq!(e.kb_view().unwrap().current, "index");
    }

    #[test]
    fn open_help_at_missing_falls_back_to_index() {
        let mut e = Editor::new();
        e.open_help_at("nonexistent:thing");
        assert_eq!(e.kb_view().unwrap().current, "index");
        assert!(e.status_msg.contains("No help node"));
    }

    #[test]
    fn open_help_reuses_existing_buffer() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.open_help_at("concept:buffer");
        let helps = e
            .buffers
            .iter()
            .filter(|b| b.kind == BufferKind::Kb)
            .count();
        assert_eq!(helps, 1);
        assert_eq!(e.kb_view().unwrap().current, "concept:buffer");
        // back_stack should show the previous node.
        assert_eq!(e.kb_view().unwrap().back_stack, vec!["index"]);
    }

    #[test]
    fn help_follow_link_navigates() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.help_next_link(); // focus first link
        let focused_target = {
            let links = e.kb_navigable_links();
            let v = e.kb_view().unwrap();
            links[v.focused_link.unwrap()].clone()
        };
        e.help_follow_link();
        assert_eq!(e.kb_view().unwrap().current, focused_target);
    }

    #[test]
    fn help_back_and_forward() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.open_help_at("concept:buffer");
        e.help_back();
        assert_eq!(e.kb_view().unwrap().current, "index");
        e.help_forward();
        assert_eq!(e.kb_view().unwrap().current, "concept:buffer");
    }

    #[test]
    fn help_close_removes_buffer() {
        let mut e = Editor::new();
        e.open_help_at("index");
        assert_eq!(e.buffers.len(), 2);
        e.help_close();
        assert!(e.buffers.iter().all(|b| b.kind != BufferKind::Kb));
        assert_eq!(e.active_buffer_idx(), 0);
    }

    #[test]
    fn help_next_prev_link_wraps() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let count = e.kb_navigable_links().len();
        assert!(count > 0);
        e.help_next_link();
        assert_eq!(e.kb_view().unwrap().focused_link, Some(0));
        e.help_prev_link();
        assert_eq!(e.kb_view().unwrap().focused_link, Some(count - 1));
    }

    #[test]
    fn kb_navigable_links_includes_backlinks() {
        let e = {
            let mut e = Editor::new();
            e.open_help_at("index");
            e
        };
        let outgoing = e.kb.primary.links_from("index");
        let incoming = e.kb.primary.links_to("index");
        assert!(!outgoing.is_empty(), "index must have outgoing links");
        assert!(!incoming.is_empty(), "index must have incoming links");

        let nav = e.kb_navigable_links();
        // Every outgoing neighbor appears somewhere in nav links.
        for target in &outgoing {
            assert!(
                nav.contains(target),
                "missing outgoing link {} in nav list",
                target
            );
        }
        // Every backlink is reachable through the combined list.
        for src in &incoming {
            assert!(nav.contains(src), "missing backlink {} in nav list", src);
        }
    }

    #[test]
    fn help_follow_link_works_for_backlink_focus() {
        let mut e = Editor::new();
        e.open_help_at("concept:buffer");
        let nav = e.kb_navigable_links();
        if nav.len() > 1 {
            let last_idx = nav.len() - 1;
            if let Some(view) = e.kb_view_mut() {
                view.focused_link = Some(last_idx);
            }
            let expected = nav[last_idx].clone();
            e.help_follow_link();
            assert_eq!(e.kb_view().unwrap().current, expected);
        }
    }

    // --- WU5: rope-backed KB buffer tests ---

    #[test]
    fn help_buffer_is_read_only() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let idx = e.active_buffer_idx();
        assert!(e.buffers[idx].read_only);
        let before = e.buffers[idx].rope().len_chars();
        let mut win = crate::window::Window::new(999, idx);
        e.buffers[idx].insert_char(&mut win, 'x');
        assert_eq!(
            e.buffers[idx].rope().len_chars(),
            before,
            "read-only buffer should reject insert_char"
        );
    }

    #[test]
    fn help_buffer_has_rope_content() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let idx = e.active_buffer_idx();
        let text: String = e.buffers[idx].rope().chars().collect();
        assert!(text.contains("index"), "rope should contain the node id");
        assert!(
            text.len() > 50,
            "rendered help should have substantial content"
        );
    }

    #[test]
    fn help_buffer_link_spans_have_valid_byte_ranges() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let view = e.kb_view().unwrap();
        let text: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        assert!(!view.rendered_links.is_empty(), "index should have links");
        for link in &view.rendered_links {
            assert!(link.byte_start < link.byte_end);
            assert!(link.byte_end <= text.len());
            let display = &text[link.byte_start..link.byte_end];
            assert!(!display.is_empty(), "link display text should not be empty");
        }
    }

    #[test]
    fn render_body_line_strips_brackets() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_body_line("see [[concept:buffer]] for details", &mut out, &mut links);
        assert_eq!(out, "see concept:buffer for details");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(
            &out[links[0].byte_start..links[0].byte_end],
            "concept:buffer"
        );
    }

    #[test]
    fn render_body_line_display_override() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_body_line("goto [[concept:buffer|the buffer]]", &mut out, &mut links);
        assert_eq!(out, "goto the buffer");
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(&out[links[0].byte_start..links[0].byte_end], "the buffer");
    }

    #[test]
    fn render_body_line_empty_target_is_plain() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_body_line("[[]] stays", &mut out, &mut links);
        assert_eq!(out, "[[]] stays");
        assert!(links.is_empty());
    }

    #[test]
    fn help_populate_after_navigation() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let text_before: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        e.open_help_at("concept:buffer");
        let text_after: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        assert_ne!(
            text_before, text_after,
            "rope should change after navigating to different node"
        );
        assert!(text_after.contains("concept:buffer"));
    }

    // --- WU6: reopen last KB buffer ---

    #[test]
    fn help_close_saves_state_for_reopen() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.open_help_at("concept:buffer");
        e.help_close();
        assert!(e.last_kb_state.is_some());
        assert_eq!(e.last_kb_state.as_ref().unwrap().current, "concept:buffer");
        assert_eq!(e.last_kb_state.as_ref().unwrap().back_stack, vec!["index"]);
    }

    #[test]
    fn help_reopen_restores_state() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.open_help_at("concept:buffer");
        e.help_close();
        e.help_reopen();
        assert_eq!(e.kb_view().unwrap().current, "concept:buffer");
        assert_eq!(e.kb_view().unwrap().back_stack, vec!["index"]);
        assert_eq!(e.active_buffer().kind, BufferKind::Kb);
    }

    #[test]
    fn help_reopen_no_previous() {
        let mut e = Editor::new();
        e.help_reopen();
        assert!(e.status_msg.contains("No previous help session"));
    }

    #[test]
    fn help_edit_source_no_source_shows_status() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.help_edit_source();
        assert!(e.status_msg.contains("No source file"));
    }

    #[test]
    fn help_edit_source_opens_file() {
        let mut e = Editor::new();
        // Insert a node with a source file
        let tmp = std::env::temp_dir().join("mae-test-edit-source.org");
        std::fs::write(&tmp, "test content").unwrap();
        let node = mae_kb::Node::new(
            "user:src-test",
            "Source Test",
            mae_kb::NodeKind::Note,
            "body",
        )
        .with_source_file(tmp.clone());
        e.kb.primary.insert(node);
        e.open_help_at("user:src-test");
        e.help_edit_source();
        // Should have opened the file
        let opened = e.buffers.iter().any(|b| {
            b.file_path()
                .map(|p| p.ends_with("mae-test-edit-source.org"))
                .unwrap_or(false)
        });
        assert!(opened, "should have opened the source file");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn describe_command_live_includes_keybindings() {
        let e = Editor::new();
        let text = e.describe_command_live("move-left");
        assert!(text.is_some());
        let text = text.unwrap();
        assert!(text.contains("movement"), "should include category");
        // The default keymaps bind h to move-left
        assert!(text.contains("normal"), "should include normal mode");
    }

    // --- KB UX: title heading scale (Fix 1) ---

    #[test]
    fn help_title_has_heading_prefix() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let text: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        assert!(
            text.starts_with("* "),
            "title should have * prefix for org heading scale, got: {}",
            &text[..text.len().min(40)]
        );
    }

    #[test]
    fn help_neighborhood_has_h2_heading() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let text: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        assert!(
            text.contains("** Neighborhood"),
            "neighborhood should use ## heading"
        );
    }

    // --- KB UX: drawer stripping (Fix 2) ---

    #[test]
    fn help_strips_properties_drawer() {
        let mut e = Editor::new();
        let node = mae_kb::Node::new(
            "user:drawer-test",
            "Drawer Test",
            mae_kb::NodeKind::Note,
            ":PROPERTIES:\n:ID: drawer-test\n:END:\nVisible body.\n",
        );
        e.kb.primary.insert(node);
        e.open_help_at("user:drawer-test");
        let text: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        assert!(
            !text.contains(":PROPERTIES:"),
            "properties drawer should be stripped"
        );
        assert!(text.contains("Visible body"), "body content should remain");
    }

    // --- KB UX: kb-view command (Fix 3) ---

    #[test]
    fn kb_view_returns_to_help_buffer() {
        let mut e = Editor::new();
        e.open_help_at("index");
        // Switch away from help
        e.display_buffer(0);
        assert_ne!(e.active_buffer().kind, BufferKind::Kb);
        // kb-view should return
        e.help_return_to_view();
        assert_eq!(e.active_buffer().kind, BufferKind::Kb);
    }

    #[test]
    fn kb_view_reopens_closed_help() {
        let mut e = Editor::new();
        e.open_help_at("concept:buffer");
        e.help_close();
        assert!(e.buffers.iter().all(|b| b.kind != BufferKind::Kb));
        e.help_return_to_view();
        assert_eq!(e.active_buffer().kind, BufferKind::Kb);
        assert_eq!(e.kb_view().unwrap().current, "concept:buffer");
    }

    #[test]
    fn kb_view_no_help_shows_status() {
        let mut e = Editor::new();
        e.help_return_to_view();
        assert!(e.status_msg.contains("No KB view"));
    }

    // --- KB UX: help heading folding (Fix 4) ---

    #[test]
    fn help_heading_cycle_folds_heading() {
        let mut e = Editor::new();
        // Insert a node with headings
        let node = mae_kb::Node::new(
            "user:fold-test",
            "Fold Test",
            mae_kb::NodeKind::Note,
            "** Section 1\nBody 1\nBody 2\n** Section 2\nBody 3\n",
        );
        e.kb.primary.insert(node);
        e.open_help_at("user:fold-test");
        let buf_idx = e.active_buffer_idx();
        // Find the ** Section 1 line (should be after title + metadata)
        let text: String = e.buffers[buf_idx].rope().chars().collect();
        let section_row = text
            .lines()
            .position(|l| l.starts_with("** Section 1"))
            .unwrap();
        e.window_mgr.focused_window_mut().cursor_row = section_row;
        e.help_heading_cycle();
        assert!(
            !e.buffers[buf_idx].folded_ranges.is_empty(),
            "heading should be folded"
        );
        // Toggle again to unfold
        e.help_heading_cycle();
        assert!(
            e.buffers[buf_idx].folded_ranges.is_empty(),
            "heading should be unfolded"
        );
    }

    #[test]
    fn help_close_all_folds_works() {
        let mut e = Editor::new();
        let node = mae_kb::Node::new(
            "user:fold-all-test",
            "Fold All",
            mae_kb::NodeKind::Note,
            "** A\nBody A\n** B\nBody B\n",
        );
        e.kb.primary.insert(node);
        e.open_help_at("user:fold-all-test");
        let buf_idx = e.active_buffer_idx();
        e.help_close_all_folds();
        assert!(
            !e.buffers[buf_idx].folded_ranges.is_empty(),
            "should have folds"
        );
        e.help_open_all_folds();
        assert!(
            e.buffers[buf_idx].folded_ranges.is_empty(),
            "should have no folds"
        );
    }

    // --- KB UX: broken link detection (Fix 5) ---

    #[test]
    fn help_broken_links_detected() {
        let mut e = Editor::new();
        let node = mae_kb::Node::new(
            "user:broken-link-test",
            "Broken Links",
            mae_kb::NodeKind::Note,
            "See [[nonexistent:target]] for info.\n",
        );
        e.kb.primary.insert(node);
        e.open_help_at("user:broken-link-test");
        let view = e.kb_view().unwrap();
        assert!(
            !view.broken_links.is_empty(),
            "should detect broken link to nonexistent:target"
        );
    }

    #[test]
    fn help_valid_links_not_broken() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let view = e.kb_view().unwrap();
        // The index node links to real nodes — none should be broken
        let valid_count = view
            .rendered_links
            .iter()
            .enumerate()
            .filter(|(i, _)| !view.broken_links.contains(i))
            .count();
        assert!(valid_count > 0, "index should have valid links");
    }

    #[test]
    fn help_follow_broken_link_fuzzy_resolves() {
        let mut e = Editor::new();
        // Create a node with a link that partially matches an existing node
        let node = mae_kb::Node::new(
            "user:fuzzy-test",
            "Fuzzy Test",
            mae_kb::NodeKind::Note,
            "See [[concept:buffer]] for info.\n",
        );
        e.kb.primary.insert(node);
        e.open_help_at("user:fuzzy-test");
        // Focus the link and follow it — should work since concept:buffer exists
        e.help_next_link();
        e.help_follow_link();
        assert_eq!(e.kb_view().unwrap().current, "concept:buffer");
    }

    // --- KB UX: hint footer (Fix 6) ---

    #[test]
    fn help_footer_shows_new_keybindings() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let text: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        assert!(text.contains("Tab: fold"), "footer should mention Tab fold");
        assert!(
            text.contains("n/p: links"),
            "footer should mention n/p links"
        );
        assert!(text.contains("e: edit"), "footer should mention edit");
    }

    // --- Bug A: close-window on sole conversation group ---

    #[test]
    fn close_window_on_sole_conversation_group_resets() {
        use crate::editor::ConversationPair;

        let mut e = Editor::new();
        // Simulate creating a conversation pair with a group layout
        let out_buf = crate::buffer::Buffer::new_conversation("*AI*");
        e.buffers.push(out_buf);
        let output_idx = e.buffers.len() - 1;
        let mut input_buf = crate::buffer::Buffer::new();
        input_buf.name = "*ai-input*".to_string();
        e.buffers.push(input_buf);
        let input_idx = e.buffers.len() - 1;
        // Create a split layout with two windows
        let area = e.default_area();
        let out_win_id = e.window_mgr.focused_id();
        e.window_mgr.focused_window_mut().buffer_idx = output_idx;
        let input_win_id = e
            .window_mgr
            .split(crate::window::SplitDirection::Horizontal, input_idx, area)
            .unwrap();
        e.window_mgr.set_focused(input_win_id);
        // Group them as a conversation pair
        e.window_mgr
            .wrap_subtree_as_group(&[out_win_id, input_win_id], "ai-chat".to_string());
        e.ai.conversation_pair = Some(ConversationPair {
            output_buffer_idx: output_idx,
            input_buffer_idx: input_idx,
            output_window_id: out_win_id,
            input_window_id: input_win_id,
        });
        assert!(
            e.window_mgr.is_in_group(input_win_id),
            "input window should be in group"
        );
        // Now close-window should tear down the conversation
        e.dispatch_builtin("close-window");
        assert!(
            e.ai.conversation_pair.is_none(),
            "conversation pair should be cleared"
        );
        assert_eq!(e.mode, crate::Mode::Normal, "should return to Normal mode");
        assert!(
            e.buffers
                .iter()
                .all(|b| b.kind != crate::BufferKind::Conversation),
            "conversation buffers should be removed"
        );
    }

    // --- Phase 3: render_kb_node_for_query typed link labels ---

    #[test]
    fn render_for_query_shows_typed_link_labels() {
        use mae_kb::query::CozoQueryLayer;
        use mae_kb::store::KbStore;
        use mae_kb::{CozoKbStore, Node, NodeKind};
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(CozoKbStore::open(tmp.path().join("test.cozo")).unwrap());
        store
            .insert_node(&Node::new(
                "lesson:nav",
                "Navigation",
                NodeKind::Lesson,
                "Learn to move.",
            ))
            .unwrap();
        store
            .insert_node(&Node::new(
                "concept:buffer",
                "Buffer",
                NodeKind::Concept,
                "A text buffer.",
            ))
            .unwrap();
        // Add a typed link: lesson:nav teaches concept:buffer
        store
            .add_typed_link("lesson:nav", "concept:buffer", "teaches", 1.0)
            .unwrap();

        let layer = CozoQueryLayer::new(store);
        let (text, links) = render_kb_node_for_query(&layer, "lesson:nav");

        // The neighborhood section should show "teaches → concept:buffer"
        assert!(
            text.contains("teaches → concept:buffer"),
            "typed link label should appear in rendered output, got:\n{}",
            text
        );
        // There should be a navigable link to concept:buffer
        assert!(
            links.iter().any(|l| l.target == "concept:buffer"),
            "concept:buffer should be a navigable link"
        );
    }

    #[test]
    fn render_for_query_backlink_shows_typed_label() {
        use mae_kb::query::CozoQueryLayer;
        use mae_kb::store::KbStore;
        use mae_kb::{CozoKbStore, Node, NodeKind};
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(CozoKbStore::open(tmp.path().join("test.cozo")).unwrap());
        store
            .insert_node(&Node::new("lesson:nav", "Navigation", NodeKind::Lesson, ""))
            .unwrap();
        store
            .insert_node(&Node::new(
                "concept:buffer",
                "Buffer",
                NodeKind::Concept,
                "A text buffer.",
            ))
            .unwrap();
        store
            .add_typed_link("lesson:nav", "concept:buffer", "teaches", 1.0)
            .unwrap();

        let layer = CozoQueryLayer::new(store);
        // Render concept:buffer — should show lesson:nav as a backlink with "teaches ←" label
        let (text, links) = render_kb_node_for_query(&layer, "concept:buffer");

        assert!(
            text.contains("teaches ← lesson:nav"),
            "backlink should show typed label, got:\n{}",
            text
        );
        assert!(
            links.iter().any(|l| l.target == "lesson:nav"),
            "lesson:nav should be a navigable backlink"
        );
    }

    #[test]
    fn render_for_query_missing_node() {
        use mae_kb::query::CozoQueryLayer;
        use mae_kb::CozoKbStore;
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(CozoKbStore::open(tmp.path().join("test.cozo")).unwrap());
        let layer = CozoQueryLayer::new(store);
        let (text, links) = render_kb_node_for_query(&layer, "nonexistent:id");

        assert!(
            text.contains("no such KB node"),
            "should show missing node message"
        );
        assert!(links.is_empty(), "no links for missing node");
    }
}
