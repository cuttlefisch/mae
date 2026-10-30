//! Typed-link CRUD + link-derived query helpers: known relationship
//! types, adding/replacing typed links, filtering by rel_type, deriving
//! links from a node's body text (ADR-030), and bulk link loading.

use super::util::{btree_params, cozo_err, dv_str, parse_link_row};
use super::*;

impl CozoKbStore {
    /// Links are parsed from the body with the same typed parser the daemon
    /// projector uses (`daemon/src/projector.rs::project_node`): ADR-030 text is
    /// truth and the `links` rows are its projection, `?rel=&w=&c=` honoured
    /// rather than every edge hardcoded to `related_to`.
    ///
    /// The graph edges a node's body declares — the ONE derivation every write
    /// path stores (the editor's `insert_node` and the daemon projector, which
    /// used to filter differently, principle #8):
    /// - typed links parsed per ADR-030 (`?rel=&w=&c=`);
    /// - external URLs dropped: a link in the TEXT, not an edge in the GRAPH (an
    ///   edge to a node that cannot exist is a permanent "broken link" in
    ///   kb_health);
    /// - one edge per `(dst, rel_type)`, the LAST in body order — what the old
    ///   per-link `:put`s left behind, kept so stored graphs do not change.
    pub fn derived_links(node: &Node) -> Vec<(String, String, f64, f64)> {
        let mut out: Vec<(String, String, f64, f64)> = Vec::new();
        for l in crate::org::parse_typed_links(&node.body, &node.id) {
            if crate::org::is_external_link_target(&l.target) {
                continue;
            }
            match out
                .iter_mut()
                .find(|(d, r, _, _)| *d == l.target && *r == l.rel_type)
            {
                Some(existing) => *existing = (l.target, l.rel_type, l.weight, l.confidence),
                None => out.push((l.target, l.rel_type, l.weight, l.confidence)),
            }
        }
        out
    }

    /// The body of [`KbStore::insert_node`]: the node row, its link
    /// replacement and its meta-member replacement in one write transaction,
    /// retried as a whole on SQLite busy contention. Derived rows are computed
    /// with the same parsers the separate `update_*_for_node` helpers use, so
    /// what is stored does not change — only that it is stored together.
    pub(super) fn write_node_atomically(&self, node: &Node) -> Result<(), KbStoreError> {
        let node_params = self.node_put_params(node)?;
        let now = DataValue::from(self.now_epoch());
        let link_rows: Vec<DataValue> = Self::derived_links(node)
            .into_iter()
            .map(|(dst, rel_type, weight, confidence)| {
                DataValue::List(vec![
                    dv_str(&node.id),
                    dv_str(&dst),
                    dv_str(&rel_type),
                    dv_str(""),
                    DataValue::from(weight),
                    DataValue::from(confidence),
                    now.clone(),
                ])
            })
            .collect();
        let member_rows: Vec<DataValue> = crate::org::parse_transclusion_directives(&node.body)
            .into_iter()
            .enumerate()
            .map(|(position, (member_id, role))| {
                DataValue::List(vec![
                    dv_str(&node.id),
                    dv_str(&member_id),
                    DataValue::from(position as i64),
                    dv_str(&role),
                ])
            })
            .collect();
        let mut params = node_params;
        params.insert("link_rows".into(), DataValue::List(link_rows));
        params.insert("member_rows".into(), DataValue::List(member_rows));
        // One script, one transaction (cozo runs the `{…}` blocks of a chained
        // script atomically). Deliberately NOT `DbInstance::multi_transaction`:
        // its commit replies before its background task releases the connection
        // and the store's write lock, so a store dropped right after a write
        // could still be held — a file opened next (backup, restore, migration)
        // then saw "database is locked".
        #[allow(unused_mut)]
        let mut script = format!(
            "{{ {} }}\n{{ {} }}\n{{ {} }}\n{{ {} }}\n{{ {} }}",
            Self::NODE_PUT_SCRIPT,
            Self::LINKS_FROM_RM_SCRIPT,
            Self::LINK_BULK_SCRIPT.replace("$rows", "$link_rows"),
            Self::META_MEMBERS_RM_SCRIPT,
            Self::META_MEMBER_BULK_SCRIPT.replace("$rows", "$member_rows"),
        );
        #[cfg(test)]
        if super::test_hooks::fail_after_node_row() {
            // A block that fails at RUN time (not when the script is compiled —
            // that would roll back before anything ran and prove nothing), after
            // the node row was written: the whole script must roll back.
            script.push_str("\n{ ?[x] <- [[1]] :assert none }");
        }
        self.run_mut_params(&script, params).map_err(cozo_err)?;
        Ok(())
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
            r#"?[src, dst, rel_type] := src = $id, *links{src, dst, rel_type}
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
                // `rel_type` deliberately stays a POST-filter while `src` is
                // pre-bound. `links` is keyed `(src, dst, rel_type)`; binding
                // src and rel_type but not dst joins positions {0, 2}, which
                // `ra.rs:1509 join_is_prefix` rejects, and cozo falls back to
                // `stored_mat_join` — a full scan. Binding the shorter,
                // contiguous prefix and filtering the rest seeks. See
                // `tests/query_plan_tests.rs::links_typed_binds_only_the_contiguous_key_prefix`.
                "?[src, dst, rel_type, display, weight, confidence] := src = $id, *links{src, dst, rel_type, display, weight, confidence}, rel_type = $rel_type",
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
