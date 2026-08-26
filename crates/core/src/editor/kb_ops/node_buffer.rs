//! ADR-092 D3/D5 — editing a KB node as its normalized org source text.
//!
//! # Why this exists
//!
//! Post-cutover a node may have **no file at all**: a detached KB's store is the
//! source of truth, and a hosted deployment never had `.org` files to begin with.
//! `kb-edit-source` already refuses in that case and tells the user to *"edit the
//! node here instead"* — which was **not true**: `BufferKind::Kb` is read-only and
//! there was no other node edit surface. That message is now honest.
//!
//! # The surface is the node's normalized org source text
//!
//! Not its rendered view, and not a file. `mae_kb::export::node_to_org` ⟷
//! `mae_kb::org::parse_org` is a verified round trip — identity on a parsed node,
//! reaching a fixed point on the first pass — which is the prerequisite that
//! makes this safe. Without it, opening a node and saving it unchanged would
//! silently drop whichever fields the serializer forgot, and it forgot six.
//!
//! # One write path
//!
//! Saving routes through `kb_update_node_with`, ADR-092 D1's sole node-content
//! mutator, so this inherits `kb_write_blocked`, owner resolution across
//! primary ∪ federated instances, the seed-node refusal and the CRDT-vs-direct
//! branch rather than re-deriving any of them.

use super::Editor;

/// Buffer-name prefix for a node edit buffer. The node id is encoded in the
/// name, the same way `*kb-narrow:META:MEMBER*` already encodes its pair.
const NODE_BUFFER_PREFIX: &str = "*kb-node:";

/// Which surface `kb-edit-source` opens for a node.
///
/// D5 requires the default to reproduce today's behaviour **exactly**: a
/// file-backed node in an attached KB keeps opening its file, byte-identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditSurface {
    /// File when there is one and it is still authoritative; the node buffer
    /// otherwise. The default.
    Auto,
    /// Always the source file — refuse when there is none.
    File,
    /// Always the node buffer, even for a file-backed node.
    Node,
}

impl EditSurface {
    pub fn from_option(value: &str) -> Self {
        match value {
            "file" => EditSurface::File,
            "node" => EditSurface::Node,
            _ => EditSurface::Auto,
        }
    }
}

/// The node id a `*kb-node:ID*` buffer is editing.
pub fn node_id_from_buffer_name(name: &str) -> Option<String> {
    let inner = name.strip_prefix(NODE_BUFFER_PREFIX)?.strip_suffix('*')?;
    (!inner.is_empty()).then(|| inner.to_string())
}

pub fn node_buffer_name(node_id: &str) -> String {
    format!("{NODE_BUFFER_PREFIX}{node_id}*")
}

impl Editor {
    pub fn kb_edit_surface(&self) -> EditSurface {
        self.get_option("kb_edit_surface")
            .map(|(v, _)| EditSurface::from_option(&v))
            .unwrap_or(EditSurface::Auto)
    }

    /// Open `node_id` for editing as org source text.
    ///
    /// Reuses an already-open buffer for the same node rather than stacking a
    /// second one — two buffers over one node would let a stale copy overwrite a
    /// fresh edit, which is the divergence the single write path exists to stop.
    pub fn kb_edit_node(&mut self, node_id: &str) -> Result<(), String> {
        let node = self
            .kb_resolve_node(node_id)
            .ok_or_else(|| format!("No KB node: {node_id}"))?;
        let text = mae_kb::export::node_to_org(&node);
        let name = node_buffer_name(node_id);

        if let Some(idx) = self.buffers.iter().position(|b| b.name == name) {
            self.display_buffer(idx);
            self.set_status(format!("Editing '{node_id}' — :w saves to the KB"));
            return Ok(());
        }

        let mut buf = crate::Buffer::new();
        buf.name = name;
        buf.insert_text_at(0, &text);
        buf.modified = false;
        self.buffers.push(buf);
        let idx = self.buffers.len() - 1;
        self.display_buffer(idx);
        self.set_status(format!("Editing '{node_id}' — :w saves to the KB"));
        Ok(())
    }

    /// Save a `*kb-node:ID*` buffer back into the KB.
    ///
    /// Returns `false` when `idx` is not a node buffer, so the ordinary file
    /// save path continues untouched.
    pub(crate) fn kb_save_node_buffer(&mut self, idx: usize) -> bool {
        let Some(node_id) = node_id_from_buffer_name(&self.buffers[idx].name) else {
            return false;
        };
        let text = self.buffers[idx].text();

        // Parse BEFORE writing anything. A body that no longer carries a
        // file-level `:ID:` would otherwise be written as an empty node —
        // the edit surface destroying the node it was opened to edit.
        let Some(parsed) = mae_kb::org::parse_org(&text) else {
            self.set_status(format!(
                "'{node_id}' not saved: the buffer has no file-level :ID: property, so \
                 there is nothing to identify the node being edited. Restore the \
                 :PROPERTIES: drawer, or close the buffer to discard."
            ));
            return true;
        };
        if parsed.id != node_id {
            self.set_status(format!(
                "'{node_id}' not saved: the buffer's :ID: now reads '{}'. Renaming a \
                 node's id is not an edit to its content — it would orphan every link \
                 pointing at it. Use :kb-create for a new node.",
                parsed.id
            ));
            return true;
        }

        match self.kb_update_node_from_source(&node_id, parsed) {
            Ok(()) => {
                self.buffers[idx].modified = false;
                self.set_status(format!("'{node_id}' saved to the KB"));
            }
            Err(e) => self.set_status(format!("'{node_id}' not saved: {e}")),
        }
        true
    }

    /// Apply a parsed node's content over the stored one, through the sole
    /// mutator (ADR-092 D1).
    fn kb_update_node_from_source(
        &mut self,
        node_id: &str,
        parsed: mae_kb::Node,
    ) -> Result<(), String> {
        self.kb_update_node_with(node_id, move |n| {
            n.title = parsed.title;
            n.body = parsed.body;
            n.tags = parsed.tags;
            n.kind = parsed.kind;
            n.todo_state = parsed.todo_state;
            n.priority = parsed.priority;
            n.aliases = parsed.aliases;
            n.properties = parsed.properties;
        })
    }

    /// A node from wherever it lives — query layer, primary, or a federated
    /// instance — mirroring the read path the KB view itself uses.
    pub(crate) fn kb_resolve_node(&self, node_id: &str) -> Option<mae_kb::Node> {
        if let Some(q) = self.kb.query_layer() {
            if let Some(n) = q.get(node_id) {
                return Some(n);
            }
        }
        if let Some(n) = self.kb.primary.get(node_id) {
            return Some(n.clone());
        }
        self.kb
            .instances
            .values()
            .find_map(|kb| kb.get(node_id).cloned())
    }
}

impl Editor {
    /// A node's provenance, from wherever the node lives.
    ///
    /// Used to keep `auto` from opening an edit buffer over protected seed
    /// content: `kb_update_node_with` would refuse the save, and a buffer that
    /// can only fail is worse than a message that says so up front.
    pub(crate) fn kb_resolve_node_source(&self, node_id: &str) -> Option<mae_kb::NodeSource> {
        self.kb_resolve_node(node_id).and_then(|n| n.source)
    }
}
