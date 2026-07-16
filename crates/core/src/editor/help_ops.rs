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
/// Render a "Related" subsection: graph-related nodes (co-citation /
/// bibliographic coupling / shared tags) that are NOT already shown as direct
/// links. This surfaces adjacent topics the Neighborhood section misses — the
/// human-facing twin of the AI's `kb_related` tool (peer parity). Returns the
/// number of related items rendered. `shown` is the set of already-rendered
/// direct-link ids; `resolve_title` maps an id to its display title.
fn render_related_block(
    out: &mut String,
    links: &mut Vec<KbLinkSpan>,
    related: &[(String, f64)],
    shown: &std::collections::HashSet<String>,
    resolve_title: impl Fn(&str) -> String,
) -> usize {
    const MAX_RELATED: usize = 8;
    let items: Vec<&String> = related
        .iter()
        .map(|(id, _)| id)
        .filter(|id| !shown.contains(*id))
        .take(MAX_RELATED)
        .collect();
    if items.is_empty() {
        return 0;
    }
    out.push_str("Related:\n");
    for id in &items {
        let title = resolve_title(id);
        out.push_str("  ~ ");
        let link_start = out.len();
        out.push_str(id);
        let link_end = out.len();
        links.push(KbLinkSpan {
            byte_start: link_start,
            byte_end: link_end,
            target: (*id).clone(),
        });
        out.push_str(&format!("  {}\n", title));
    }
    items.len()
}

/// One neighborhood link ready to render — already correlated with its
/// optional typed relationship label (see call sites for how each render
/// path builds this cheaply, without per-item title resolution).
struct NeighborLink {
    id: String,
    rel_label: Option<String>,
}

/// Cap on outgoing/incoming links actually rendered (title-resolved) per
/// direction, regardless of a node's true degree.
///
/// Regression fix: this Neighborhood block used to be duplicated three
/// times (the `cmd:` live-render branch below, `render_kb_node_for_query`,
/// `render_kb_node_with_store`), each doing an UNCAPPED per-link title
/// resolution — for a true hub node (`index`: 1300+ edges), that's a
/// synchronous multi-second stall on every single click, reported live.
/// `render_kb_node_with_store` additionally did an O(n) linear `.find()`
/// per link to correlate typed relationship data — O(n^2) on top of the
/// uncapped O(n). One shared, capped renderer closes both: the duplication
/// (principle #8 — this is exactly how the cap could exist in one path and
/// not the others) and the unbounded cost (principle #9).
const MAX_NEIGHBORHOOD_LINKS: usize = 50;

/// Render one direction (`Outgoing:` or `Backlinks (N):`) of a node's
/// Neighborhood block. `header` carries the TRUE total count (even when
/// capped) so degree stays visible; only the first `MAX_NEIGHBORHOOD_LINKS`
/// entries get title-resolved and rendered, with a "... (N more)" note for
/// the rest — mirroring `render_related_block`'s cap-with-count-note shape.
fn render_neighborhood_links(
    out: &mut String,
    links: &mut Vec<KbLinkSpan>,
    header: &str,
    entries: &[NeighborLink],
    arrow: &str,
    resolve_title: impl Fn(&str) -> String,
) {
    if entries.is_empty() {
        return;
    }
    out.push_str(header);
    let total = entries.len();
    for entry in entries.iter().take(MAX_NEIGHBORHOOD_LINKS) {
        let title_text = resolve_title(&entry.id);
        out.push_str("  ");
        if let Some(rel) = &entry.rel_label {
            out.push_str(rel);
            out.push(' ');
            out.push_str(arrow);
            out.push(' ');
        } else {
            out.push_str(arrow);
            out.push(' ');
        }
        let link_start = out.len();
        out.push_str(&entry.id);
        let link_end = out.len();
        links.push(KbLinkSpan {
            byte_start: link_start,
            byte_end: link_end,
            target: entry.id.clone(),
        });
        out.push_str(&format!("  {}\n", title_text));
    }
    if total > MAX_NEIGHBORHOOD_LINKS {
        out.push_str(&format!(
            "  ... ({} more)\n",
            total - MAX_NEIGHBORHOOD_LINKS
        ));
    }
}

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

    // Body — strip property drawers + leading #+keywords, then scan the
    // whole filtered body for [[...]] links (not per already-split line —
    // a link whose display text spans a line break must still resolve).
    let filtered_body = strip_kb_body_noise(&node.body);
    render_kb_body(&filtered_body, &mut out, &mut links);

    // Neighborhood — query layer returns typed links directly
    let outgoing = query.links_from(node_id);
    let incoming = query.links_to(node_id);

    // Graph-related nodes that aren't already direct links (Phase 4).
    let shown: std::collections::HashSet<String> = outgoing
        .iter()
        .map(|l| l.dst.clone())
        .chain(incoming.iter().map(|l| l.src.clone()))
        .collect();
    let related = query.related(node_id, 24);
    let has_related = related.iter().any(|(id, _)| !shown.contains(id));

    if !outgoing.is_empty() || !incoming.is_empty() || has_related {
        out.push('\n');
        out.push_str("** Neighborhood\n");
    }
    let resolve_title = |id: &str| {
        query
            .get(id)
            .map(|n| n.title)
            .unwrap_or_else(|| "(missing)".to_string())
    };
    let to_entries = |typed: &[mae_kb::store::Link], id_of: fn(&mae_kb::store::Link) -> &str| {
        typed
            .iter()
            .map(|l| NeighborLink {
                id: id_of(l).to_string(),
                rel_label: (l.rel_type != "references").then(|| l.rel_type.clone()),
            })
            .collect::<Vec<_>>()
    };
    render_neighborhood_links(
        &mut out,
        &mut links,
        "Outgoing:\n",
        &to_entries(&outgoing, |l| &l.dst),
        "→",
        resolve_title,
    );
    render_neighborhood_links(
        &mut out,
        &mut links,
        &format!("Backlinks ({}):\n", incoming.len()),
        &to_entries(&incoming, |l| &l.src),
        "←",
        resolve_title,
    );
    render_related_block(&mut out, &mut links, &related, &shown, resolve_title);

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

    let filtered_body = strip_kb_body_noise(&node.body);
    render_kb_body(&filtered_body, &mut out, &mut links);

    let (outgoing_typed, incoming_typed) = if let Some(st) = store {
        let out_links = st.links_from(node_id).unwrap_or_default();
        let in_links = st.links_to(node_id).unwrap_or_default();
        (Some(out_links), Some(in_links))
    } else {
        (None, None)
    };

    let outgoing_ids = kb.links_from(node_id);
    let incoming_ids = kb.links_to(node_id);

    // Graph-related nodes that aren't already direct links (Phase 4).
    let shown: std::collections::HashSet<String> = outgoing_ids
        .iter()
        .chain(incoming_ids.iter())
        .cloned()
        .collect();
    let related = kb.related(node_id, 24);
    let has_related = related.iter().any(|(id, _)| !shown.contains(id));

    if !outgoing_ids.is_empty() || !incoming_ids.is_empty() || has_related {
        out.push('\n');
        out.push_str("** Neighborhood\n");
    }
    // Build id -> rel_label maps ONCE (was a per-link linear `.find()` over
    // the full typed list — O(n^2) on a hub node's full link set).
    let rel_map = |typed: &Option<Vec<mae_kb::store::Link>>,
                   id_of: fn(&mae_kb::store::Link) -> &str|
     -> std::collections::HashMap<String, String> {
        typed
            .iter()
            .flatten()
            .filter(|l| l.rel_type != "references")
            .map(|l| (id_of(l).to_string(), l.rel_type.clone()))
            .collect()
    };
    let outgoing_rel = rel_map(&outgoing_typed, |l| &l.dst);
    let incoming_rel = rel_map(&incoming_typed, |l| &l.src);
    let to_entries = |ids: &[String], rel: &std::collections::HashMap<String, String>| {
        ids.iter()
            .map(|id| NeighborLink {
                id: id.clone(),
                rel_label: rel.get(id).cloned(),
            })
            .collect::<Vec<_>>()
    };
    let resolve_title_str = |id: &str| resolve_title(id).unwrap_or_else(|| "(missing)".to_string());
    render_neighborhood_links(
        &mut out,
        &mut links,
        "Outgoing:\n",
        &to_entries(&outgoing_ids, &outgoing_rel),
        "→",
        resolve_title_str,
    );
    render_neighborhood_links(
        &mut out,
        &mut links,
        &format!("Backlinks ({}):\n", incoming_ids.len()),
        &to_entries(&incoming_ids, &incoming_rel),
        "←",
        resolve_title_str,
    );
    render_related_block(&mut out, &mut links, &related, &shown, resolve_title_str);

    out.push('\n');
    out.push_str(
        "Tab: fold · n/p: links · Enter: follow · e: edit · C-o/C-i: back/fwd · q: close\n",
    );

    (out, links)
}

/// Strip `:PROPERTIES:`/`:LOGBOOK:` drawers and leading `#+`-keyword lines
/// from a KB node body, joining the kept lines back with `\n`. Hoisted out
/// of the render functions into its own pre-pass so link-scanning
/// (`render_kb_body`) can run over the *whole* filtered body at once instead
/// of one already-split line at a time — needed for a link whose display
/// text spans a line break (#301/#302).
pub(crate) fn strip_kb_body_noise(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_drawer = false;
    let mut kept_lines = 0usize;
    for line in body.lines() {
        let trimmed = line.trim();
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
        // Strip #+keyword lines near top (already shown in header metadata).
        if trimmed.starts_with("#+") && kept_lines < 4 {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        kept_lines += 1;
    }
    out
}

/// Render a KB node body (or any already-filtered multi-line text): resolve
/// every `[[...]]` link to plain display text + a `KbLinkSpan` (byte-accurate
/// against `out`), copying everything else through unchanged.
///
/// Whole-string-aware via `mae_kb::org::next_link_span` (ADR-030: "the
/// parser is the canonical projector"), so a link whose display text spans a
/// `\n` is still recognized — the bug class behind #301/#302.
///
/// Accepts two link grammars, since node bodies aren't uniform in this
/// codebase: native/primary nodes store the raw ADR-030 `[[TARGET][DISPLAY]]`
/// form directly (`kb_update` writes user-provided body text verbatim, no
/// rewrite step), while federated/org-dir-imported nodes were pre-rewritten
/// at import time to the older `[[TARGET|DISPLAY]]` pipe form
/// (`org::rewrite_links`). Try `|` first (the unambiguous legacy separator
/// for already-imported bodies), falling back to the canonical `][` split.
fn render_kb_body(body: &str, out: &mut String, links: &mut Vec<KbLinkSpan>) {
    let code_ranges = mae_kb::compute_code_block_ranges(body);
    let mut cursor = 0usize;
    while let Some(m) = mae_kb::org::next_link_span(body, cursor, &code_ranges) {
        // Emit literal text before the link (char-boundary-safe: `cursor`
        // and `m.full_start` only ever land on ASCII '[' bytes or a prior
        // match's end, both always valid UTF-8 boundaries).
        out.push_str(&body[cursor..m.full_start]);

        let (target_raw, display_opt) = match m.inner.find('|') {
            Some(bar) => (m.inner[..bar].trim(), Some(m.inner[bar + 1..].trim())),
            None => match m.inner.find("][") {
                Some(sep) => (m.inner[..sep].trim(), Some(m.inner[sep + 2..].trim())),
                None => (m.inner.trim(), None),
            },
        };
        // Strip the ADR-030 `?query` relationship metadata from the target:
        // it's authored CRDT truth (read by the projector + AI peer), never
        // shown. The `#fragment` (before `?`) is kept for node resolution.
        let target = match target_raw.find('?') {
            Some(p) => target_raw[..p].trim_end(),
            None => target_raw,
        };
        // Bare links (no display) show the clean node id, not the query.
        let display = display_opt.unwrap_or(target);
        if !target.is_empty() {
            let link_start = out.len();
            out.push_str(display);
            let link_end = out.len();
            links.push(KbLinkSpan {
                byte_start: link_start,
                byte_end: link_end,
                target: target.to_string(),
            });
        } else {
            // Empty target: not a real link — keep the literal markup
            // (matches the historical stray-`[[]]` behavior).
            out.push_str("[[");
            out.push_str(m.inner);
            out.push_str("]]");
        }
        cursor = m.full_end;
    }
    out.push_str(&body[cursor..]);
}

impl Editor {
    /// TUI textual fallback for the native KB graph view (`BufferKind::Graph`,
    /// Part C Phase 1): delegates to the EXISTING KB "** Neighborhood"
    /// rendering machinery (`render_kb_node_for_query`) for the graph's
    /// center node, rather than rendering `GraphView.scene`'s positions
    /// (which only the GUI's Skia canvas consumes) as text. Lives here
    /// (not `graph_view_ops.rs`) specifically so it can call the private
    /// `render_kb_node_for_query` directly without widening that
    /// function's visibility.
    pub fn render_graph_view_as_text(&self) -> String {
        let Some(center) = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::Graph)
            .and_then(|b| b.graph_view())
            .and_then(|gv| gv.center_node.clone())
        else {
            return "(KB graph view: no center node yet — open with :kb-graph-view-open)\n"
                .to_string();
        };
        // Same query-layer-miss-falls-through-to-in-memory reasoning as
        // `kb_contains_any`'s doc comment.
        if self.kb.query_layer().is_some_and(|q| q.contains(&center)) {
            return render_kb_node_for_query(self.kb.query_layer().unwrap(), &center).0;
        }
        if let Some(kb) = self.kb_for_node(&center) {
            let local = &self.kb.primary;
            let federated = &self.kb.instances;
            return render_kb_node_with_store(
                kb,
                &center,
                |id| {
                    local.get(id).map(|n| n.title.clone()).or_else(|| {
                        federated
                            .values()
                            .find_map(|fkb| fkb.get(id).map(|n| n.title.clone()))
                    })
                },
                self.kb.store.as_deref(),
            )
            .0;
        }
        format!(
            "* KB Graph — {}\n(no KB query layer available; graph data unavailable in this build)\n",
            center
        )
    }

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
    ///
    /// `pub(crate)` (not module-private) so link-following outside the
    /// `*KB*` view (`mouse_ops::handle_link_click`, `org_ops::org_open_link`
    /// — #293) can reuse the same existence check the KB view already uses,
    /// instead of a third, divergent resolver.
    pub(crate) fn kb_contains_any(&self, id: &str) -> bool {
        self.kb_get_node_anywhere(id).is_some()
    }

    /// Resolve a node title across local + federated KBs.
    fn kb_resolve_title(&self, id: &str) -> Option<String> {
        self.kb_get_node_anywhere(id).map(|n| n.title)
    }

    /// Get the KnowledgeBase that contains a given node ID (local first, then federated).
    ///
    /// `pub(crate)` so `kb_preview_ops.rs`'s `fetch_kb_preview_content` (Part
    /// D, KB-link hover preview) can reuse the same lookup instead of a
    /// second divergent resolver — mirrors why `kb_contains_any` above is
    /// `pub(crate)`.
    pub(crate) fn kb_for_node(&self, id: &str) -> Option<&mae_kb::KnowledgeBase> {
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
                // Parse the live text for links (whole-string scan — see
                // render_kb_body — so a link whose display text spans a
                // line break is still recognized).
                render_kb_body(live_text.as_str(), &mut out, &mut links);
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
                // Graph-related nodes that aren't already direct links (Phase 4).
                let shown: std::collections::HashSet<String> = outgoing_links
                    .iter()
                    .map(|l| l.dst.clone())
                    .chain(incoming_links.iter().map(|l| l.src.clone()))
                    .collect();
                let related = if let Some(q) = self.kb.query_layer() {
                    q.related(&node_id, 24)
                } else {
                    self.kb.primary.related(&node_id, 24)
                };
                let has_related = related.iter().any(|(id, _)| !shown.contains(id));
                if !outgoing_links.is_empty() || !incoming_links.is_empty() || has_related {
                    out.push('\n');
                    out.push_str("** Neighborhood\n");
                }
                let resolve_title = |id: &str| {
                    self.kb_resolve_title(id)
                        .unwrap_or_else(|| "(missing)".to_string())
                };
                let to_entries =
                    |typed: &[mae_kb::store::Link], id_of: fn(&mae_kb::store::Link) -> &str| {
                        typed
                            .iter()
                            .map(|l| NeighborLink {
                                id: id_of(l).to_string(),
                                rel_label: (l.rel_type != "references").then(|| l.rel_type.clone()),
                            })
                            .collect::<Vec<_>>()
                    };
                render_neighborhood_links(
                    &mut out,
                    &mut links,
                    "Outgoing:\n",
                    &to_entries(&outgoing_links, |l| &l.dst),
                    "→",
                    resolve_title,
                );
                render_neighborhood_links(
                    &mut out,
                    &mut links,
                    &format!("Backlinks ({}):\n", incoming_links.len()),
                    &to_entries(&incoming_links, |l| &l.src),
                    "←",
                    resolve_title,
                );
                render_related_block(&mut out, &mut links, &related, &shown, resolve_title);
                out.push('\n');
                out.push_str(
                    "Tab: fold · n/p: links · Enter: follow · e: edit · C-o/C-i: back/fwd · q: close\n",
                );
                (out, links)
            // Query-layer-miss-falls-through-to-in-memory: see
            // `kb_contains_any`'s doc comment. `render_kb_node_for_query`
            // does its own `query.get()` internally with no such fallback,
            // so the caller (here) must pre-check `contains` and route to
            // the in-memory-backed `render_kb_node_with_store` path when
            // the query layer's projection hasn't caught up.
            } else if self.kb.query_layer().is_some_and(|q| q.contains(&node_id)) {
                render_kb_node_for_query(self.kb.query_layer().unwrap(), &node_id)
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
        // Same query-layer-miss-falls-through-to-in-memory reasoning as
        // the `cmd:` branch above.
        } else if self.kb.query_layer().is_some_and(|q| q.contains(&node_id)) {
            render_kb_node_for_query(self.kb.query_layer().unwrap(), &node_id)
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

    /// Find the KB link at an arbitrary `(row, col)` position in the
    /// FOCUSED window's buffer, if that buffer is showing a KB node
    /// (`BufferKind::Kb`) and the position falls inside one of its
    /// `KbView.rendered_links` byte ranges.
    ///
    /// Read-only: unlike `help_follow_link`, this never navigates or
    /// mutates `KbView.focused_link` — it's a pure lookup for the KB-link
    /// hover preview (Part D, `kb_preview_ops.rs`), which needs to know
    /// "is the cursor on a link right now" without following it. Mirrors
    /// `help_cursor_byte_offset`'s row/col → byte-offset conversion (same
    /// technique `mouse_ops::try_link_follow_at` uses to generalize
    /// cursor-based link-follow to arbitrary mouse positions), generalized
    /// to an arbitrary `(row, col)` instead of only the live cursor.
    pub(crate) fn kb_link_at(&self, row: usize, col: usize) -> Option<KbLinkSpan> {
        let buf_idx = self.window_mgr.focused_window().buffer_idx;
        let buf = self.buffers.get(buf_idx)?;
        if buf.kind != BufferKind::Kb {
            return None;
        }
        let view = buf.kb_view()?;
        if view.rendered_links.is_empty() {
            return None;
        }
        let rope = buf.rope();
        if rope.len_lines() == 0 {
            return None;
        }
        let row = row.min(rope.len_lines().saturating_sub(1));
        let line_start = rope.line_to_byte(row);
        let line_len = rope.line(row).len_bytes();
        let col_bytes = col.min(line_len);
        let byte_offset = line_start + col_bytes;
        view.rendered_links
            .iter()
            .find(|l| byte_offset >= l.byte_start && byte_offset < l.byte_end)
            .cloned()
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

    /// Resolve a KB node's CURRENT source file path, re-deriving against the
    /// owning instance's current `org_dir` if the stored (possibly stale)
    /// `source_file` no longer exists on disk (#303: `source_file` is
    /// stamped once at import time and never re-validated, so it can drift
    /// from the instance's *current* `org_dir` — a moved directory, a
    /// non-canonical path at an earlier register call, etc.). Returns
    /// `None` only if the node has no source file at all (e.g. a native or
    /// promoted node) — never a stale, unresolvable path.
    ///
    /// Shared by `help_edit_source` and `resolve_kb_link` (#293's
    /// `source-file` follow mode) so there's one re-derivation
    /// implementation, not two.
    pub(crate) fn kb_node_source_file(&self, node_id: &str) -> Option<std::path::PathBuf> {
        let path = self
            .kb
            .primary
            .get(node_id)
            .or_else(|| self.kb.instances.values().find_map(|kb| kb.get(node_id)))
            .and_then(|n| n.source_file.clone())?;
        if path.exists() {
            return Some(path);
        }
        if let Some(Some(uuid)) = self.kb_owner_of(node_id) {
            if let Some(instance) = self.kb.registry.find(&uuid) {
                if let Some(resolved) =
                    mae_kb::federation::resolve_stale_source_file(&instance.org_dir, &path)
                {
                    return Some(resolved);
                }
            }
        }
        Some(path)
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

        match self.kb_node_source_file(&node_id) {
            Some(path) => {
                let path_str = path.display().to_string();
                self.open_file(&path_str);
            }
            None => {
                self.set_status(format!("No source file for '{}'", node_id));
            }
        }
    }

    /// Resolve a link target that may refer to a KB graph node, honoring
    /// `kb_link_follow_mode` (#293, principle #7 — user-configurable, not
    /// hardcoded): `"kb-view"` opens the rendered `*KB*` view (the existing
    /// federation-aware entry point `:help <id>` already uses); `"source-
    /// file"` jumps straight to the node's current source file instead
    /// (reusing `kb_node_source_file`'s #303 re-derivation rather than
    /// trusting a possibly-moved path verbatim).
    ///
    /// Returns `true` if `target` resolved as a KB node and was handled
    /// here; `false` if the caller should fall through to its own non-KB
    /// behavior (`handle_link_click`'s file/URL opener, `org_open_link`'s
    /// same-buffer heading jump) — e.g. a link to a not-yet-created daily
    /// note, which isn't a KB node yet.
    pub(crate) fn resolve_kb_link(&mut self, target: &str) -> bool {
        let kb_target = target.strip_prefix("id:").unwrap_or(target);
        if !self.kb_contains_any(kb_target) {
            return false;
        }
        if self.kb_link_follow_mode == "source-file" {
            if let Some(path) = self.kb_node_source_file(kb_target) {
                self.open_file(path.display().to_string());
                return true;
            }
            // No source file at all (native or already-promoted node) —
            // fall back to the KB view rather than doing nothing.
        }
        self.open_help_at(kb_target);
        true
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
    fn render_graph_view_as_text_no_center_before_open() {
        let e = Editor::new();
        let text = e.render_graph_view_as_text();
        assert!(text.contains("no center node"));
    }

    #[test]
    fn render_graph_view_as_text_falls_back_to_in_memory_when_no_query_layer() {
        // A plain `Editor::new()` test fixture has no `KbQueryLayer` wired
        // (that's assembled by the binary's bootstrap, not the core
        // constructor) — confirms the in-memory fallback renders the
        // node's real content (query-layer-miss-falls-through-to-in-memory,
        // see `kb_contains_any`'s doc comment) rather than a spurious
        // "unavailable" placeholder when the node genuinely exists.
        let mut e = Editor::new();
        e.kb_graph_view_open(Some("index".to_string()), Some(1));
        assert!(e.kb.query_layer().is_none());
        let text = e.render_graph_view_as_text();
        assert!(text.contains("index"));
        assert!(
            !text.contains("no KB query layer"),
            "the in-memory fallback must render real content, not the unavailable placeholder: {text}"
        );
    }

    #[test]
    fn render_graph_view_as_text_reports_unavailable_when_center_resolves_nowhere() {
        // Genuine graceful-degradation case: no query layer AND the center
        // node isn't in any in-memory KB either — must still degrade
        // gracefully rather than panicking.
        let mut e = Editor::new();
        if let Some(idx) = e.buffers.iter().position(|b| b.kind == BufferKind::Graph) {
            if let Some(gv) = e.buffers[idx].graph_view_mut() {
                gv.center_node = Some("nonexistent:ghost".to_string());
            }
        } else {
            e.kb_graph_view_open(Some("index".to_string()), Some(1));
            let idx = e
                .buffers
                .iter()
                .position(|b| b.kind == BufferKind::Graph)
                .unwrap();
            e.buffers[idx].graph_view_mut().unwrap().center_node =
                Some("nonexistent:ghost".to_string());
        }
        assert!(e.kb.query_layer().is_none());
        let text = e.render_graph_view_as_text();
        assert!(text.contains("no KB query layer"));
    }

    #[test]
    fn open_help_at_finds_a_node_via_in_memory_fallback_when_the_query_layer_lags_behind() {
        // Direct regression test for the reported bug: clicking a graph
        // node showed "(no such KB node: ...)" even though the node
        // demonstrably existed (findable via search, resolvable as a
        // graph-view center) — because `kb_contains_any`/`open_help_at`
        // trusted a `Some` query layer exclusively, with no fallback, and
        // the query layer (a CozoDB PROJECTION, ADR-029) had not yet
        // caught up to a node the in-memory KB already had.
        //
        // Reproduces that exact shape: an empty (freshly-opened, never
        // populated) real `CozoQueryLayer` wired as the query layer —
        // `.contains()` on it is unconditionally false for everything —
        // while `kb.primary` has the node. Before the fix, `open_help_at`
        // would report "No help node" and fall back to "index" instead of
        // opening the real node.
        use mae_kb::query::CozoQueryLayer;
        use mae_kb::CozoKbStore;
        use std::sync::Arc;

        let mut e = ed_with_kb_node_for_help_tests("scheme:gc-collect!", "Scheme: gc-collect!");
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(CozoKbStore::open(tmp.path().join("empty.cozo")).unwrap());
        let layer: Arc<dyn mae_kb::query::KbQueryLayer> = Arc::new(CozoQueryLayer::new(store));
        e.kb.set_daemon_query_layer(Some(layer));
        assert!(
            !e.kb.query_layer().unwrap().contains("scheme:gc-collect!"),
            "sanity: the empty query layer must NOT contain the node (simulating a lagging projection)"
        );

        e.open_help_at("scheme:gc-collect!");

        assert_eq!(
            e.active_buffer().kb_view().unwrap().current,
            "scheme:gc-collect!",
            "must open the real node via the in-memory fallback, not silently redirect to index"
        );
        let text = e.active_buffer().rope().to_string();
        assert!(
            !text.contains("no such KB node") && !text.contains("No help node"),
            "buffer content must be the real node's content, not an error placeholder: {text}"
        );
        assert!(text.contains("Scheme: gc-collect!"));
    }

    fn ed_with_kb_node_for_help_tests(id: &str, title: &str) -> Editor {
        let mut e = Editor::new();
        e.kb.primary.insert(mae_kb::Node::new(
            id,
            title,
            mae_kb::NodeKind::Concept,
            "body text",
        ));
        e
    }

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
    fn kb_navigation_in_one_window_does_not_bleed_into_a_sibling_window() {
        // Reproduces the reported bug: two split windows each showing a
        // different (non-builtin) KB node used to silently alias the SAME
        // singleton `*KB*` buffer, so navigating one window's node bled into
        // the other. Non-builtin nodes must now each get their own buffer.
        let mut e = Editor::new();
        e.kb_create_node("user:node-a", "Node A", "a body", mae_kb::NodeKind::Note)
            .unwrap();
        e.kb_create_node("user:node-b", "Node B", "b body", mae_kb::NodeKind::Note)
            .unwrap();
        e.kb_create_node("user:node-c", "Node C", "c body", mae_kb::NodeKind::Note)
            .unwrap();

        // ensure_kb_buffer_idx must give distinct non-builtin nodes distinct buffers.
        let buf_a = e.ensure_kb_buffer_idx("user:node-a");
        let buf_b = e.ensure_kb_buffer_idx("user:node-b");
        assert_ne!(
            buf_a, buf_b,
            "distinct non-builtin nodes must get distinct buffers"
        );

        // Split (focus stays on the original window, per `split_and_focus`), point
        // the original window at node A's buffer and the new one at node B's.
        e.dispatch_builtin("split-vertical");
        assert_eq!(e.window_mgr.window_count(), 2);
        e.switch_to_buffer(buf_a);
        e.dispatch_builtin("focus-right");
        e.switch_to_buffer(buf_b);
        assert_eq!(e.kb_view().unwrap().current, "user:node-b");

        // Navigate the FOCUSED window (still the new/right one) to a third node.
        e.open_help_at("user:node-c");
        assert_eq!(e.kb_view().unwrap().current, "user:node-c");
        let buf_c = e.active_buffer_idx();
        assert_ne!(
            buf_c, buf_b,
            "navigating to a new node must not mutate node-b's buffer"
        );

        // Move focus back to the original window: node A must be untouched.
        e.dispatch_builtin("focus-left");
        assert_eq!(e.active_buffer_idx(), buf_a);
        assert_eq!(
            e.kb_view().unwrap().current,
            "user:node-a",
            "the sibling window's node must not change when the other window navigates"
        );
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
    fn help_follow_link_after_multiline_display_link_no_byte_drift() {
        // #302: a link earlier in the body whose display text spans a line
        // break used to leave raw brackets in the rendered text (#301),
        // shifting byte offsets for every link after it. Once #301 is
        // fixed, a link positioned after a multi-line-display link must
        // still be followable from the cursor position it's actually
        // rendered at.
        let mut e = Editor::new();
        e.kb.primary.insert(mae_kb::Node::new(
            "user:target-a",
            "Target A",
            mae_kb::NodeKind::Note,
            "",
        ));
        e.kb.primary.insert(mae_kb::Node::new(
            "user:target-b",
            "Target B",
            mae_kb::NodeKind::Note,
            "",
        ));
        e.kb.primary.insert(mae_kb::Node::new(
            "user:source",
            "Source",
            mae_kb::NodeKind::Note,
            "First [[user:target-a|a\nlink]] then [[user:target-b|another link]].",
        ));

        e.open_help_at("user:source");
        let buf_idx = e
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Kb)
            .unwrap();

        // The rendered buffer also lists both targets again under
        // "Outgoing:" in the Neighborhood section, so don't assume an exact
        // total count — find each target's *first* (body) occurrence,
        // which is what appears before the neighborhood section.
        let rendered_links = e.kb_view().unwrap().rendered_links.clone();
        let link_a = rendered_links
            .iter()
            .find(|l| l.target == "user:target-a")
            .expect("target-a link recognized");
        let link_b = rendered_links
            .iter()
            .find(|l| l.target == "user:target-b")
            .expect("target-b link recognized");
        assert!(
            link_a.byte_end <= link_b.byte_start,
            "the body's first link byte range must not overlap the second: {:?} vs {:?}",
            link_a,
            link_b
        );

        // Position the cursor precisely on the second link's rendered text
        // and follow it directly (no help_next_link focus-cycling).
        let target_byte = link_b.byte_start;
        let rope = e.buffers[buf_idx].rope();
        let row = rope.byte_to_line(target_byte);
        let col = target_byte - rope.line_to_byte(row);
        {
            let win = e.window_mgr.focused_window_mut();
            win.cursor_row = row;
            win.cursor_col = col;
        }
        e.help_follow_link();
        assert_eq!(
            e.kb_view().unwrap().current,
            "user:target-b",
            "cursor on the second link's visible text must resolve to its real target"
        );
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
        // Every outgoing neighbor up to the cap appears somewhere in nav
        // links (see MAX_NEIGHBORHOOD_LINKS — "index" is a true hub with
        // 1000+ edges; rendering/title-resolving every single one was a
        // confirmed multi-second UI freeze on every click, so only the
        // first MAX_NEIGHBORHOOD_LINKS per direction are ever navigable).
        for target in outgoing.iter().take(MAX_NEIGHBORHOOD_LINKS) {
            assert!(
                nav.contains(target),
                "missing outgoing link {} in nav list",
                target
            );
        }
        for src in incoming.iter().take(MAX_NEIGHBORHOOD_LINKS) {
            assert!(nav.contains(src), "missing backlink {} in nav list", src);
        }
    }

    #[test]
    fn kb_populate_buffer_caps_neighborhood_links_for_a_hub_node() {
        // Direct regression test for the reported freeze: a node with far
        // more than MAX_NEIGHBORHOOD_LINKS backlinks must still populate
        // fast (bounded work) and show an overflow note, not silently
        // truncate with no indication there's more.
        let mut e = Editor::new();
        e.kb.primary.insert(mae_kb::Node::new(
            "hub",
            "Hub",
            mae_kb::NodeKind::Concept,
            "",
        ));
        for i in 0..(MAX_NEIGHBORHOOD_LINKS + 10) {
            e.kb.primary.insert(mae_kb::Node::new(
                format!("leaf{i}"),
                format!("Leaf {i}"),
                mae_kb::NodeKind::Concept,
                "[[hub]]",
            ));
        }
        e.open_help_at("hub");
        let text = e.buffers[e.active_buffer_idx()].text();
        let backlinks_header = format!("Backlinks ({}):", MAX_NEIGHBORHOOD_LINKS + 10);
        assert!(
            text.contains(&backlinks_header),
            "header must show the TRUE total count even when capped: {text}"
        );
        assert!(
            text.contains("... (10 more)"),
            "must note the overflow count: {text}"
        );
        let nav = e.kb_navigable_links();
        assert_eq!(
            nav.iter().filter(|id| id.starts_with("leaf")).count(),
            MAX_NEIGHBORHOOD_LINKS,
            "only the capped number of backlinks may be individually navigable"
        );
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
    fn render_kb_body_strips_brackets() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_kb_body("see [[concept:buffer]] for details", &mut out, &mut links);
        assert_eq!(out, "see concept:buffer for details");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(
            &out[links[0].byte_start..links[0].byte_end],
            "concept:buffer"
        );
    }

    #[test]
    fn render_kb_body_display_override() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_kb_body("goto [[concept:buffer|the buffer]]", &mut out, &mut links);
        assert_eq!(out, "goto the buffer");
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(&out[links[0].byte_start..links[0].byte_end], "the buffer");
    }

    #[test]
    fn render_kb_body_strips_query_metadata() {
        // ADR-030 (Phase C): the in-target `?rel=…&w=…` metadata is hidden from the
        // rendered view — only the display shows; the span target is the clean id.
        let mut out = String::new();
        let mut links = Vec::new();
        render_kb_body(
            "see [[concept:buffer?rel=teaches&w=0.8|the buffer]] now",
            &mut out,
            &mut links,
        );
        assert_eq!(out, "see the buffer now");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(&out[links[0].byte_start..links[0].byte_end], "the buffer");

        // A bare link (no |display) shows the clean node id, query stripped.
        let mut out2 = String::new();
        let mut links2 = Vec::new();
        render_kb_body("[[concept:x?rel=cites]]", &mut out2, &mut links2);
        assert_eq!(out2, "concept:x");
        assert_eq!(links2[0].target, "concept:x");

        // Fragment (before the query) is retained for node resolution.
        let mut out3 = String::new();
        let mut links3 = Vec::new();
        render_kb_body(
            "[[concept:rope#arch?rel=implements|ropes]]",
            &mut out3,
            &mut links3,
        );
        assert_eq!(out3, "ropes");
        assert_eq!(links3[0].target, "concept:rope#arch");
    }

    #[test]
    fn render_kb_body_empty_target_is_plain() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_kb_body("[[]] stays", &mut out, &mut links);
        assert_eq!(out, "[[]] stays");
        assert!(links.is_empty());
    }

    // --- #301/#302: whole-string-aware render_kb_body ---

    #[test]
    fn render_kb_body_multiline_display_text_no_raw_brackets() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_kb_body(
            "see [[concept:buffer|the\nbuffer]] now",
            &mut out,
            &mut links,
        );
        assert_eq!(out, "see the\nbuffer now");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(&out[links[0].byte_start..links[0].byte_end], "the\nbuffer");
        assert!(!out.contains("[["));
        assert!(!out.contains("]]"));
    }

    #[test]
    fn render_kb_body_native_bracket_bracket_grammar() {
        // Native/primary nodes store the raw ADR-030 `[[TARGET][DISPLAY]]`
        // form directly (`kb_update` never rewrites it) — render_kb_body
        // must accept this grammar too, not just the legacy federated-import
        // pipe form.
        let mut out = String::new();
        let mut links = Vec::new();
        render_kb_body("goto [[concept:buffer][the buffer]]", &mut out, &mut links);
        assert_eq!(out, "goto the buffer");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(&out[links[0].byte_start..links[0].byte_end], "the buffer");
    }

    #[test]
    fn render_kb_body_native_bracket_bracket_multiline_display() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_kb_body(
            "see [[concept:buffer][the\nbuffer]] now",
            &mut out,
            &mut links,
        );
        assert_eq!(out, "see the\nbuffer now");
        assert_eq!(links.len(), 1);
        assert!(!out.contains("[["));
    }

    #[test]
    fn render_kb_body_back_to_back_links_no_byte_collision() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_kb_body("[[concept:a|A]][[concept:b|B]]", &mut out, &mut links);
        assert_eq!(out, "AB");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "concept:a");
        assert_eq!(&out[links[0].byte_start..links[0].byte_end], "A");
        assert_eq!(links[1].target, "concept:b");
        assert_eq!(&out[links[1].byte_start..links[1].byte_end], "B");
        assert!(links[0].byte_end <= links[1].byte_start);
    }

    #[test]
    fn render_kb_body_bracket_lookalike_before_real_link() {
        // A `]]`-lookalike (unrelated bracket pair) appearing in prose
        // before the real link must not confuse the scanner.
        let mut out = String::new();
        let mut links = Vec::new();
        render_kb_body(
            "array[i]] then [[concept:buffer|buffer]]",
            &mut out,
            &mut links,
        );
        assert_eq!(out, "array[i]] then buffer");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "concept:buffer");
    }

    #[test]
    fn render_kb_body_unterminated_bracket_is_literal() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_kb_body("see [[concept:buffer no close", &mut out, &mut links);
        assert_eq!(out, "see [[concept:buffer no close");
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
    fn help_edit_source_survives_org_dir_move() {
        // #303 adversarial repro: the node's `source_file` is stamped once
        // at import time and never re-validated. If the owning instance's
        // org_dir later correctly points somewhere else (the directory was
        // moved and re-registered), the stale absolute `source_file` no
        // longer resolves — `help_edit_source` must re-derive against the
        // instance's *current* org_dir instead of ENOENT-ing on the stale
        // path.
        let old_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            old_dir.path().join("note1.org"),
            ":PROPERTIES:\n:ID: moved-note\n:END:\n#+title: Moved Note\n\nBody.\n",
        )
        .unwrap();

        let mut e = Editor::new();
        e.config_dir_override = Some(old_dir.path().join("cfgdir"));
        e.data_dir_override = Some(old_dir.path().join("datadir"));
        let result = e
            .kb_register("MovedNotes", old_dir.path())
            .expect("registration should succeed");
        assert!(
            e.kb.instances[&result.uuid].get("moved-note").is_some(),
            "note should have imported from the original location"
        );

        // Simulate the directory having been moved to a new location and
        // the instance re-pointed at it (the registry's org_dir is now
        // correct; the node's `source_file` is not).
        let new_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            new_dir.path().join("note1.org"),
            ":PROPERTIES:\n:ID: moved-note\n:END:\n#+title: Moved Note\n\nBody.\n",
        )
        .unwrap();
        e.kb.registry.find_mut(&result.uuid).unwrap().org_dir =
            new_dir.path().canonicalize().unwrap();
        // The stale `source_file` must actually be gone — otherwise
        // `help_edit_source`'s existence check never falls through to
        // re-derivation, and this test would pass for the wrong reason.
        std::fs::remove_file(old_dir.path().join("note1.org")).unwrap();

        e.open_help_at("moved-note");
        e.help_edit_source();

        let opened = e.buffers.iter().find(|b| {
            b.file_path()
                .map(|p| p.ends_with("note1.org"))
                .unwrap_or(false)
        });
        let opened =
            opened.expect("should have re-derived and opened the file at its new location");
        assert!(
            opened
                .file_path()
                .unwrap()
                .starts_with(new_dir.path().canonicalize().unwrap()),
            "must open the file under the CURRENT org_dir, not the stale one"
        );
        assert!(
            !e.status_msg.to_lowercase().contains("error"),
            "must not report an error once re-derivation succeeds, got: {}",
            e.status_msg
        );
    }

    #[test]
    fn help_edit_source_after_promotion_shows_no_source() {
        // #303's concrete regression test: once a federated node has been
        // promoted to primary (kb_promote_node), it no longer carries
        // `source_file` at all — `help_edit_source` must report an honest
        // "No source file" instead of the ENOENT that used to occur when a
        // *stale* source_file path was trusted verbatim.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("note1.org"),
            ":PROPERTIES:\n:ID: promote-src-test\n:END:\n#+title: Promote Test\n\nBody.\n",
        )
        .unwrap();

        let mut e = Editor::new();
        e.config_dir_override = Some(dir.path().join("cfgdir"));
        e.data_dir_override = Some(dir.path().join("datadir"));
        e.kb_register("PromoteSrcTest", dir.path()).unwrap();
        e.kb_promote_node("promote-src-test").unwrap();

        e.open_help_at("promote-src-test");
        e.help_edit_source();

        assert!(
            e.status_msg.contains("No source file"),
            "expected an honest 'no source file' status, got: {}",
            e.status_msg
        );
        assert!(
            !e.status_msg.to_lowercase().contains("error"),
            "must not surface a raw ENOENT/error for a promoted node, got: {}",
            e.status_msg
        );
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

    #[test]
    fn help_renders_related_section_for_coupled_node() {
        // seed -> hub and coupled -> hub: `coupled` is bibliographically
        // coupled to `seed` but NOT a direct link, so it should appear under
        // "Related:" rather than the direct-link lists (Phase 2/4 peer parity).
        let mut e = Editor::new();
        e.kb.primary.insert(mae_kb::Node::new(
            "user:seed",
            "Seed",
            mae_kb::NodeKind::Note,
            "points to [[user:hub]]",
        ));
        e.kb.primary.insert(mae_kb::Node::new(
            "user:coupled",
            "Coupled",
            mae_kb::NodeKind::Note,
            "also points to [[user:hub]]",
        ));
        e.kb.primary.insert(mae_kb::Node::new(
            "user:hub",
            "Hub",
            mae_kb::NodeKind::Note,
            "",
        ));

        e.open_help_at("user:seed");
        let text: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        assert!(text.contains("Related:"), "should render a Related section");
        let related_section = text.split("Related:").nth(1).unwrap_or("");
        assert!(
            related_section.contains("user:coupled"),
            "coupled node should appear under Related, got:\n{}",
            text
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
    fn render_for_query_multiline_display_text_no_raw_brackets() {
        use mae_kb::query::CozoQueryLayer;
        use mae_kb::store::KbStore;
        use mae_kb::{CozoKbStore, Node, NodeKind};
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(CozoKbStore::open(tmp.path().join("test.cozo")).unwrap());
        store
            .insert_node(&Node::new(
                "lesson:wrap",
                "Wrap test",
                NodeKind::Lesson,
                "See [[concept:buffer|the\nbuffer]] and then [[concept:rope|the rope]] too.",
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
        store
            .insert_node(&Node::new(
                "concept:rope",
                "Rope",
                NodeKind::Concept,
                "A rope.",
            ))
            .unwrap();

        let layer = CozoQueryLayer::new(store);
        let (text, links) = render_kb_node_for_query(&layer, "lesson:wrap");

        assert!(
            !text.contains("[[") && !text.contains("]]"),
            "no raw link markup should leak, got:\n{}",
            text
        );
        let targets: Vec<_> = links.iter().map(|l| l.target.as_str()).collect();
        assert!(targets.contains(&"concept:buffer"));
        assert!(targets.contains(&"concept:rope"));
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
