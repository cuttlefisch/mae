//! `impl KbStore for CozoKbStore` — the trait implementation. The core
//! CRUD/FTS/links/CRDT/pending-update methods are implemented directly
//! here; the CozoDB-specific extensions (typed links, graph, blocks,
//! agenda, versioning, vector search, health) are thin 1–3 line
//! delegations to the inherent `CozoKbStore` methods defined in the
//! sibling query-domain modules.

use super::util::{btree_params, cozo_err, dv_str, parse_link_row, row_to_node};
use super::*;

// ---------------------------------------------------------------------------
// KbStore trait implementation
// ---------------------------------------------------------------------------

impl KbStore for CozoKbStore {
    fn insert_node(&self, node: &Node) -> Result<(), KbStoreError> {
        self.run_mut_params(Self::NODE_PUT_SCRIPT, self.node_put_params(node)?)
            .map_err(cozo_err)?;
        self.update_links_for_node(node)?;
        Ok(())
    }

    fn update_node(&self, node: &Node) -> Result<(), KbStoreError> {
        self.insert_node(node)
    }

    fn delete_node(&self, id: &str) -> Result<(), KbStoreError> {
        // Use :rm (not :delete) — :rm removes entire rows, :delete only clears values
        self.run_mut_params(
            "?[id] <- [[$id]]\n:rm nodes {id}",
            btree_params([("id", dv_str(id))]),
        )
        .map_err(cozo_err)?;

        // Remove links from this node
        self.run_mut_params(
            "?[src, dst, rel_type] := *links{src, dst, rel_type}, src = $id\n:rm links {src, dst, rel_type}",
            btree_params([("id", dv_str(id))]),
        )
        .map_err(cozo_err)?;

        Ok(())
    }

    fn get_node(&self, id: &str) -> Result<Option<Node>, KbStoreError> {
        let result = self
            .run_immut_params(
                r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                    aliases_json, properties_json, crdt_doc, has_crdt]
                    := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                              aliases_json, properties_json, crdt_doc, has_crdt},
                    id = $id"#,
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;

        if let Some(row) = result.rows.first() {
            let node = row_to_node(row)?;
            // Sled backend may leave ghost rows after :rm — treat as absent
            if node.title.is_empty() && node.body.is_empty() && node.tags.is_empty() {
                Ok(None)
            } else {
                Ok(Some(node))
            }
        } else {
            Ok(None)
        }
    }

    fn list_ids(&self, prefix: Option<&str>) -> Result<Vec<String>, KbStoreError> {
        // Filter out ghost rows (title is empty string after :rm — defensive)
        let result = match prefix {
            Some(p) => self
                .run_immut_params(
                    r#"?[id] := *nodes{id, title}, starts_with(id, $prefix), title != """#,
                    btree_params([("prefix", dv_str(p))]),
                )
                .map_err(cozo_err)?,
            None => self
                .run_immut(r#"?[id] := *nodes{id, title}, title != """#)
                .map_err(cozo_err)?,
        };

        let mut ids: Vec<String> = result
            .rows
            .iter()
            .filter_map(|row| row.first()?.get_str().map(|s| s.to_string()))
            .collect();
        ids.sort();
        Ok(ids)
    }

    fn fts_search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError> {
        if query.is_empty() {
            // Empty query: return node IDs (no ranking), bounded by `limit` —
            // an unbounded scan here was reachable from the AI `kb_search` tool.
            let result = self
                .run_immut(&format!(
                    "?[id] := *nodes{{id, title}}, title != '' :limit {limit}"
                ))
                .map_err(cozo_err)?;
            return Ok(result
                .rows
                .iter()
                .filter_map(|row| {
                    Some(SearchHit {
                        id: row.first()?.get_str()?.to_string(),
                        score: 0.0,
                    })
                })
                .collect());
        }

        // Use Tantivy FTS index for ranked search.
        // Fetch extra candidates to allow for post-query filtering
        // (guards against stale FTS index entries).
        let fetch_k = limit * 3 + 10;
        let result = self
            .run_immut_params(
                &format!(
                    r#"?[id, score] := ~nodes:fts{{id | query: $query, k: {fetch_k}, bind_score: score}}"#
                ),
                btree_params([("query", dv_str(query))]),
            )
            .map_err(cozo_err)?;

        // Post-query verification: check each candidate's actual content still
        // matches (defensive against stale FTS index entries). Previously this
        // did one full `get_node` per candidate (N+1 — up to limit*3+10 full
        // node deserializes just to read title+body). Instead bulk-fetch
        // title+body for all candidates in ONE query, then verify in Rust,
        // preserving the FTS score order.
        let candidate_ids: Vec<DataValue> = result
            .rows
            .iter()
            .filter_map(|row| row.first().and_then(|v| v.get_str()).map(dv_str))
            .collect();

        let mut content: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::with_capacity(candidate_ids.len());
        if !candidate_ids.is_empty() {
            let fetched = self
                .run_immut_params(
                    "?[id, title, body] := *nodes{id, title, body}, is_in(id, $ids)",
                    btree_params([("ids", DataValue::List(candidate_ids))]),
                )
                .map_err(cozo_err)?;
            for row in &fetched.rows {
                let (Some(id), Some(title), Some(body)) = (
                    row.first().and_then(|v| v.get_str()),
                    row.get(1).and_then(|v| v.get_str()),
                    row.get(2).and_then(|v| v.get_str()),
                ) else {
                    continue;
                };
                content.insert(id.to_string(), (title.to_string(), body.to_string()));
            }
        }

        let query_lower = query.to_lowercase();
        let query_terms: Vec<&str> = query_lower.split_whitespace().collect();
        let mut hits = Vec::new();
        for row in &result.rows {
            let Some(id) = row.first().and_then(|v| v.get_str()) else {
                continue;
            };
            let score = row.get(1).and_then(|v| v.get_float()).unwrap_or(0.0);
            if let Some((title, body)) = content.get(id) {
                let text = format!("{title} {body}").to_lowercase();
                if query_terms.iter().any(|term| text.contains(term)) {
                    hits.push(SearchHit {
                        id: id.to_string(),
                        score,
                    });
                    if hits.len() >= limit {
                        break;
                    }
                }
            }
        }
        Ok(hits)
    }

    fn add_link(&self, src: &str, dst: &str, display: Option<&str>) -> Result<(), KbStoreError> {
        let now = self.now_epoch();
        self.run_mut_params(
            r#"?[src, dst, rel_type, display, weight, confidence, created_at] <- [[$src, $dst, "related_to", $display, 1.0, 1.0, $now]]
            :put links {src, dst, rel_type => display, weight, confidence, created_at}"#,
            btree_params([
                ("src", dv_str(src)),
                ("dst", dv_str(dst)),
                ("display", dv_str(display.unwrap_or(""))),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    fn remove_link(&self, src: &str, dst: &str) -> Result<(), KbStoreError> {
        self.run_mut_params(
            r#"
            ?[src, dst, rel_type] := *links{src, dst, rel_type}, src = $src, dst = $dst
            :rm links {src, dst, rel_type}
            "#,
            btree_params([("src", dv_str(src)), ("dst", dv_str(dst))]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[src, dst, rel_type, display, weight, confidence] := *links{src, dst, rel_type, display, weight, confidence}, src = $id",
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| parse_link_row(row))
            .collect())
    }

    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[src, dst, rel_type, display, weight, confidence] := *links{src, dst, rel_type, display, weight, confidence}, dst = $id",
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| parse_link_row(row))
            .collect())
    }

    fn push_pending_update(
        &self,
        kb_id: &str,
        node_id: &str,
        update: &[u8],
    ) -> Result<(), KbStoreError> {
        let id = self.next_pending_id()?;
        let now = self.now_epoch();
        self.run_mut_params(
            r#"?[id, kb_id, node_id, update_bytes, created_at] <- [[$id, $kb_id, $node_id, $update_bytes, $now]]
            :put pending_updates {id => kb_id, node_id, update_bytes, created_at}"#,
            btree_params([
                ("id", DataValue::from(id)),
                ("kb_id", dv_str(kb_id)),
                ("node_id", dv_str(node_id)),
                ("update_bytes", DataValue::Bytes(update.to_vec())),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    fn drain_pending_updates(&self) -> Result<Vec<PendingUpdate>, KbStoreError> {
        let result = self
            .run_immut(
                "?[id, kb_id, node_id, update_bytes] := *pending_updates{id, kb_id, node_id, update_bytes} :order id",
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                let rowid = row.first()?.get_int()?;
                let kb_id = row.get(1)?.get_str()?.to_string();
                let node_id = row.get(2)?.get_str()?.to_string();
                let update_bytes = row.get(3)?.get_bytes()?.to_vec();
                Some(PendingUpdate {
                    rowid,
                    kb_id,
                    node_id,
                    update_bytes,
                })
            })
            .collect())
    }

    fn ack_pending_update(&self, id: i64) -> Result<(), KbStoreError> {
        self.run_mut_params(
            r#"?[id] <- [[$id]]
            :rm pending_updates {id}"#,
            btree_params([("id", DataValue::from(id))]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    fn load_all(&self) -> Result<Vec<Node>, KbStoreError> {
        let query = r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                    aliases_json, properties_json, crdt_doc, has_crdt]
                    := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                              aliases_json, properties_json, crdt_doc, has_crdt},
                    title != ""
                    :order id"#;
        // B-5: a malformed / short-arity stored row (e.g. one left by an older
        // schema version or a previously-broken write path) makes the ENTIRE cozo
        // query fail at bind time ("tuple bound by variable 'title' is too short")
        // — before the per-row skip loop below ever runs. Propagating that error
        // here previously aborted the caller (e.g. `kb_join`) and, on the main
        // thread, tripped the 10s stall watchdog. Degrade to an empty load (logged
        // at ERROR for repair visibility) so the editor keeps running: this is the
        // same observable state as a genuinely empty store, which every caller
        // already handles, and strictly safer than a hard error. (Moving this
        // query off the UI thread is the deeper concurrency-#1 fix, tracked
        // separately.)
        let result = match self.run_immut(query) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "KB store: node load query failed — returning empty load (store may need repair); editor continues without stalling");
                return Ok(Vec::new());
            }
        };

        let mut nodes = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            // ADR-019 / B-5: tolerate a malformed row — skip it (with a warning)
            // instead of aborting the entire load, which previously errored and
            // stalled the editor's main thread on a single bad-arity row.
            match row_to_node(row) {
                Ok(node) => nodes.push(node),
                Err(e) => {
                    tracing::warn!(error = %e, "KB store: skipping malformed node row");
                }
            }
        }

        // `row_to_node` never sets `source_file` — the `nodes` relation has no
        // such column, only `source_files` (file -> node_ids) does. Reconstruct
        // it here so every `load_all` caller (fresh instance open at startup,
        // `:kb-reimport`, migration) gets a correct `source_file`, not just the
        // in-memory `KnowledgeBase` that did the original ingest.
        match self.source_file_by_node_id() {
            Ok(source_files) => {
                for node in &mut nodes {
                    if let Some(path) = source_files.get(&node.id) {
                        node.source_file = Some(path.clone());
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "KB store: failed to reconstruct source_file index — nodes will report no source file");
            }
        }
        Ok(nodes)
    }

    fn save_all(&self, nodes: &[&Node]) -> Result<(), KbStoreError> {
        // Clear existing data
        self.run_mut(
            r#"
            ?[id] := *nodes{id}
            :rm nodes {id}
            "#,
        )
        .map_err(cozo_err)?;
        self.run_mut(
            r#"
            ?[src, dst, rel_type] := *links{src, dst, rel_type}
            :rm links {src, dst, rel_type}
            "#,
        )
        .map_err(cozo_err)?;

        // Insert all nodes
        for node in nodes {
            self.insert_node(node)?;
        }
        Ok(())
    }

    // --- Trait overrides for CozoDB-specific features ---

    fn add_typed_link(
        &self,
        src: &str,
        dst: &str,
        rel_type: &str,
        weight: f64,
    ) -> Result<(), KbStoreError> {
        CozoKbStore::add_typed_link(self, src, dst, rel_type, weight)
    }

    fn links_typed(&self, id: &str, rel_type: &str) -> Result<Vec<Link>, KbStoreError> {
        CozoKbStore::links_typed(self, id, rel_type)
    }

    fn known_rel_types(&self) -> Result<std::collections::HashSet<String>, KbStoreError> {
        CozoKbStore::known_rel_types(self)
    }

    fn shortest_path(&self, from: &str, to: &str) -> Result<Vec<String>, KbStoreError> {
        CozoKbStore::shortest_path(self, from, to)
    }

    fn neighborhood(&self, id: &str, depth: u32) -> Result<SubGraph, KbStoreError> {
        CozoKbStore::neighborhood(self, id, depth)
    }

    fn related(&self, id: &str, limit: usize) -> Result<Vec<(String, f64)>, KbStoreError> {
        CozoKbStore::related(self, id, limit)
    }

    fn raw_query(&self, script: &str) -> Result<(Vec<String>, Vec<Vec<String>>), KbStoreError> {
        CozoKbStore::raw_query(self, script)
    }

    fn meta_members(&self, meta_id: &str) -> Result<Vec<MetaMember>, KbStoreError> {
        CozoKbStore::meta_members(self, meta_id)
    }

    fn add_meta_member(
        &self,
        meta_id: &str,
        member_id: &str,
        position: i32,
        role: &str,
    ) -> Result<(), KbStoreError> {
        CozoKbStore::add_meta_member(self, meta_id, member_id, position, role)
    }

    fn remove_meta_member(&self, meta_id: &str, member_id: &str) -> Result<(), KbStoreError> {
        CozoKbStore::remove_meta_member(self, meta_id, member_id)
    }

    fn compose_meta_body(&self, meta_id: &str) -> Result<String, KbStoreError> {
        CozoKbStore::compose_meta_body(self, meta_id)
    }

    fn get_blocks(&self, parent_id: &str) -> Result<Vec<Block>, KbStoreError> {
        CozoKbStore::get_blocks(self, parent_id)
    }

    fn get_block(&self, parent_id: &str, idx: usize) -> Result<Option<Block>, KbStoreError> {
        CozoKbStore::get_block(self, parent_id, idx)
    }

    fn agenda_query(&self, filter: &AgendaFilter) -> Result<Vec<Node>, KbStoreError> {
        CozoKbStore::agenda_query(self, filter)
    }

    fn node_history(&self, id: &str, limit: usize) -> Result<Vec<NodeVersion>, KbStoreError> {
        CozoKbStore::node_history(self, id, limit)
    }

    fn restore_version(&self, id: &str, version: i64) -> Result<(), KbStoreError> {
        CozoKbStore::restore_version(self, id, version)
    }

    fn store_embedding(&self, id: &str, model: &str, vec: &[f32]) -> Result<(), KbStoreError> {
        CozoKbStore::store_embedding(self, id, model, vec)
    }

    fn vector_search(&self, vec: &[f32], k: usize) -> Result<Vec<VectorHit>, KbStoreError> {
        CozoKbStore::vector_search(self, vec, k)
    }

    fn graphrag_search(&self, vec: &[f32], k: usize) -> Result<Vec<VectorHit>, KbStoreError> {
        CozoKbStore::graphrag_search(self, vec, k)
    }

    fn health_report(&self) -> Result<HealthReport, KbStoreError> {
        CozoKbStore::health_report(self)
    }

    fn detect_reimport_stale_files(&self) -> Result<Vec<ReimportStaleFile>, KbStoreError> {
        CozoKbStore::detect_reimport_stale_files(self)
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError> {
        CozoKbStore::id_title_pairs(self, prefix)
    }

    fn backend_name(&self) -> &str {
        "cozo"
    }

    fn db_path(&self) -> &Path {
        &self.path
    }
}
