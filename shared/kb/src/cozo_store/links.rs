//! Typed-link CRUD + link-derived query helpers: known relationship
//! types, adding/replacing typed links, filtering by rel_type, deriving
//! links from a node's body text (ADR-030), and bulk link loading.

use super::util::{btree_params, cozo_err, dv_str, parse_link_row};
use super::*;

impl CozoKbStore {
    /// Insert or replace node links by parsing the body (ADR-030: text is truth, this is
    /// the derived projection). Uses the same typed parse + `replace_node_links` the
    /// daemon projector uses (`daemon/src/projector.rs::project_node`) — previously this
    /// called the older untyped `parse_links`, which doesn't strip a link's `?query`
    /// string and hardcoded every edge to `rel_type="related_to"`, so an AI-authored
    /// typed link (`[[id?rel=X&w=Y][display]]`) written via `kb_update` in ordinary
    /// single-user usage produced a dangling/mistyped graph edge. ADR-031 §5 already
    /// states enrichment is supposed to work in-process without a daemon — this closes
    /// that conformance gap rather than adding new design.
    pub(super) fn update_links_for_node(&self, node: &Node) -> Result<(), KbStoreError> {
        let links: Vec<(String, String, f64, f64)> =
            crate::org::parse_typed_links(&node.body, &node.id)
                .into_iter()
                .map(|l| (l.target, l.rel_type, l.weight, l.confidence))
                .collect();
        self.replace_node_links(&node.id, &links)
    }
}

// ---------------------------------------------------------------------------
// Typed relationship extensions (CozoDB-specific)
// ---------------------------------------------------------------------------

impl CozoKbStore {
    /// Query all known relationship type names from the `rel_types` relation.
    /// Returns a set of type names (e.g., "teaches", "implements", "references").
    pub fn known_rel_types(&self) -> Result<std::collections::HashSet<String>, KbStoreError> {
        let result = self
            .run_immut("?[name] := *rel_types{name}")
            .map_err(cozo_err)?;
        Ok(result
            .rows
            .iter()
            .filter_map(|row| row.first()?.get_str().map(|s| s.to_string()))
            .collect())
    }
    /// Add a typed link between nodes with confidence score.
    pub fn add_typed_link(
        &self,
        src: &str,
        dst: &str,
        rel_type: &str,
        weight: f64,
    ) -> Result<(), KbStoreError> {
        // Strip fragment (e.g., "concept:buffer#rope-internals" → "concept:buffer")
        let dst_clean = dst.split('#').next().unwrap_or(dst);
        self.add_typed_link_with_confidence(src, dst_clean, rel_type, weight, 1.0)
    }
    /// Add a typed link with explicit confidence (0.0–1.0).
    /// AI-generated links should use lower confidence values.
    pub fn add_typed_link_with_confidence(
        &self,
        src: &str,
        dst: &str,
        rel_type: &str,
        weight: f64,
        confidence: f64,
    ) -> Result<(), KbStoreError> {
        let now = self.now_epoch();
        self.run_mut_params(
            r#"?[src, dst, rel_type, display, weight, confidence, created_at] <- [[$src, $dst, $rel_type, "", $weight, $confidence, $now]]
            :put links {src, dst, rel_type => display, weight, confidence, created_at}"#,
            btree_params([
                ("src", dv_str(src)),
                ("dst", dv_str(dst)),
                ("rel_type", dv_str(rel_type)),
                ("weight", DataValue::from(weight)),
                ("confidence", DataValue::from(confidence)),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }
    /// Replace ALL of a node's outgoing links with the given typed links (ADR-030
    /// projection): clear every link from `src`, then insert each
    /// `(dst, rel_type, weight, confidence)`. Used by the daemon projector to wire the
    /// typed graph parsed from a node's source text, superseding the generic links
    /// `insert_node` wires. Idempotent — re-projecting the same node yields the same set.
    pub fn replace_node_links(
        &self,
        src: &str,
        links: &[(String, String, f64, f64)],
    ) -> Result<(), KbStoreError> {
        self.run_mut_params(
            r#"?[src, dst, rel_type] := *links{src, dst, rel_type}, src = $id
               :rm links {src, dst, rel_type}"#,
            btree_params([("id", dv_str(src))]),
        )
        .map_err(cozo_err)?;
        for (dst, rel_type, weight, confidence) in links {
            self.add_typed_link_with_confidence(src, dst, rel_type, *weight, *confidence)?;
        }
        Ok(())
    }
    /// Query links filtered by relationship type.
    pub fn links_typed(&self, id: &str, rel_type: &str) -> Result<Vec<Link>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[src, dst, rel_type, display, weight, confidence] := *links{src, dst, rel_type, display, weight, confidence}, src = $id, rel_type = $rel_type",
                btree_params([("id", dv_str(id)), ("rel_type", dv_str(rel_type))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| parse_link_row(row))
            .collect())
    }
    /// Seed typed relationships between known seed nodes.
    ///
    /// Since v0.13.0, content relationships (lesson→concept, concept→concept)
    /// are expressed as inline typed links in org files and extracted by the
    /// org parser during ingestion. This function now only seeds relationships
    /// that can't be expressed in org files (code-generated nodes like cmd:*).
    ///
    /// Idempotent — uses :put (upsert) so re-running is safe.
    pub fn seed_typed_relationships(&self) -> Result<usize, KbStoreError> {
        let now = self.now_epoch();
        // Only code-generated relationships remain here.
        // Content relationships (lesson↔concept, concept↔concept, tutorial chains)
        // are now inline typed links in assets/manual/*.org files.
        let relationships: Vec<(&str, &str, &str, f64)> = vec![
            // Index categorizes — kept because index.org links are plain links
            // and these establish the top-level graph structure.
            ("index", "concept:buffer", "categorizes", 1.0),
            ("index", "concept:mode", "categorizes", 1.0),
            ("index", "concept:ai-as-peer", "categorizes", 1.0),
            ("index", "concept:knowledge-base", "categorizes", 1.0),
            ("index", "concept:scheme-api", "categorizes", 1.0),
            ("index", "concept:debugging", "categorizes", 1.0),
        ];

        let count = relationships.len();
        for (src, dst, rel_type, weight) in &relationships {
            self.run_mut_params(
                r#"?[src, dst, rel_type, display, weight, confidence, created_at] <- [[$src, $dst, $rel_type, "", $weight, 1.0, $now]]
                :put links {src, dst, rel_type => display, weight, confidence, created_at}"#,
                btree_params([
                    ("src", dv_str(src)),
                    ("dst", dv_str(dst)),
                    ("rel_type", dv_str(rel_type)),
                    ("weight", DataValue::from(*weight)),
                    ("now", DataValue::from(now)),
                ]),
            )
            .map_err(cozo_err)?;
        }

        Ok(count)
    }
    /// Load all links from CozoDB.
    pub fn load_all_links(&self) -> Result<Vec<Link>, KbStoreError> {
        let result = self
            .run_immut(
                r#"?[src, dst, rel_type, display, weight, confidence]
                   := *links{src, dst, rel_type, display, weight, confidence}
                   :order src, dst"#,
            )
            .map_err(cozo_err)?;

        let mut links = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            if let Some(link) = parse_link_row(row) {
                links.push(link);
            }
        }
        Ok(links)
    }
}
