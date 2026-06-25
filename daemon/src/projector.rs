//! KB projector — deterministic projection of CRDT node docs into the CozoDB query
//! store (ADR-029 / ADR-030).
//!
//! The CRDT (`KbNodeDoc`) is the source of truth; CozoDB is a derived projection. The
//! **structural projection is a pure function of the CRDT state**: parse a node's
//! source text → a cozo node + its links + FTS. Because the parse is
//! deterministic, every peer with the same converged CRDT derives a byte-identical
//! graph (the ADR-029 determinism contract). This is the seam the change feed
//! (`doc_store.apply_update`) drives, covering hub + p2p uniformly.

use mae_kb::{CozoKbStore, KbStore, Node, NodeKind, NodeSource};

/// Project a single KB node doc — the `KbNodeDoc` yrs state stored at `kb:{node_id}` —
/// into the cozo query store: materialize the node (title/body/tags/kind) + its body
/// links + FTS via `insert_node` (which parses `[[…]]` links and maintains the index).
/// Idempotent: re-projecting the same state upserts the same node and replaces its
/// links. Typed-link metadata (rel_type/weight/confidence from an extended in-text
/// grammar) lands with ADR-030 in Phase C; B1 is the structural backbone.
pub fn project_node(store: &CozoKbStore, node_id: &str, state: &[u8]) -> Result<(), String> {
    let doc = mae_sync::kb::KbNodeDoc::from_bytes(state)
        .map_err(|e| format!("parse node doc '{node_id}': {e}"))?;
    // Kind is a cozo-only projection field (the CRDT carries only content); derive it
    // deterministically from the id namespace.
    let node = Node::from_crdt_doc(&doc, kind_from_id(node_id), NodeSource::Federation);
    store
        .insert_node(&node)
        .map_err(|e| format!("project node '{node_id}': {e}"))?;
    Ok(())
}

/// Derive a node's kind from its id namespace (e.g. `concept:x` → Concept), defaulting
/// to Note. A deterministic rule so the projection converges across peers; richer
/// kind handling (from in-text org metadata) lands with ADR-030 (Phase C).
fn kind_from_id(node_id: &str) -> NodeKind {
    match node_id.split(':').next().unwrap_or("") {
        "concept" => NodeKind::Concept,
        "cmd" | "command" => NodeKind::Command,
        "lesson" => NodeKind::Lesson,
        "tutorial" => NodeKind::Tutorial,
        "category" => NodeKind::Category,
        "meta" => NodeKind::Meta,
        _ => NodeKind::Note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_kb::store::SearchHit;

    #[test]
    fn project_node_materializes_node_links_and_fts() {
        let store = CozoKbStore::open_mem().unwrap();
        let doc = mae_sync::kb::KbNodeDoc::new(
            "concept:rope",
            "Rope",
            "The rope buffer structure. See [[concept:buffer]] for usage.",
            &["alpha".to_string()],
        );
        project_node(&store, "concept:rope", &doc.encode()).unwrap();

        // The node is materialized with kind derived from the namespace.
        let n = store.get_node("concept:rope").unwrap().unwrap();
        assert_eq!(n.title, "Rope");
        assert_eq!(n.kind, NodeKind::Concept);
        assert!(n.tags.contains(&"alpha".to_string()));

        // FTS finds it.
        let hits: Vec<SearchHit> = store.fts_search("rope buffer", 10).unwrap();
        assert!(
            hits.iter().any(|h| h.id == "concept:rope"),
            "FTS should find the node"
        );

        // The body link is projected into the graph.
        let links = store.links_from("concept:rope").unwrap();
        assert!(
            links.iter().any(|l| l.dst == "concept:buffer"),
            "the body link should be projected into the graph, got: {links:?}"
        );
    }

    #[test]
    fn projection_is_deterministic_across_stores() {
        // Same CRDT state ⇒ identical projected node + links on two independent stores
        // (the ADR-029 determinism contract, at the single-node level).
        let doc = mae_sync::kb::KbNodeDoc::new(
            "concept:x",
            "X",
            "links [[concept:a]] and [[concept:b]]",
            &[],
        );
        let state = doc.encode();

        let project_all = |store: &CozoKbStore| -> (String, Vec<String>) {
            project_node(store, "concept:x", &state).unwrap();
            let title = store.get_node("concept:x").unwrap().unwrap().title;
            let mut dsts: Vec<String> = store
                .links_from("concept:x")
                .unwrap()
                .into_iter()
                .map(|l| l.dst)
                .collect();
            dsts.sort();
            (title, dsts)
        };

        let a = project_all(&CozoKbStore::open_mem().unwrap());
        let b = project_all(&CozoKbStore::open_mem().unwrap());
        assert_eq!(a, b, "the structural projection must be deterministic");
        assert_eq!(a.1, vec!["concept:a".to_string(), "concept:b".to_string()]);
    }
}
