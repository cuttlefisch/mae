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

impl CozoKbStore {
    /// Per-id fallback for [`KbStore::load_all`] when the bulk 13-column bind
    /// fails. Queries only `id` — a 1-column bind, which cannot hit the
    /// short-tuple error — then materialises each node individually via
    /// `get_node`, skipping ids that no longer resolve (the tombstones).
    fn load_all_per_id(&self) -> Result<Vec<Node>, KbStoreError> {
        let (_cols, rows) = self.raw_query(r#"?[id] := *nodes{id}"#)?;
        let mut nodes = Vec::with_capacity(rows.len());
        let mut skipped = 0usize;
        for row in rows {
            let Some(raw) = row.into_iter().next() else {
                continue;
            };
            let id = raw.trim_matches('"');
            match self.get_node(id) {
                Ok(Some(node)) => nodes.push(node),
                Ok(None) => skipped += 1,
                Err(e) => {
                    tracing::warn!(id = %id, error = %e, "KB store: skipping unreadable node during per-id load");
                    skipped += 1;
                }
            }
        }
        if skipped > 0 {
            tracing::warn!(
                skipped,
                loaded = nodes.len(),
                "KB store: per-id load skipped tombstoned/unreadable rows"
            );
        }
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(nodes)
    }
}

impl CozoKbStore {
    /// Shared body of [`KbStore::get_node`] and [`KbStore::get_node_light`]:
    /// run a single-key lookup and apply the ghost-row guard.
    ///
    /// @ai-caution: [kb-query] `query` MUST pre-bind `id` (`id = $id,
    /// *nodes{id, ...}`), not post-filter it (`*nodes{id, ...}, id = $id`).
    /// The post-filter form compiles to a full relation scan — measured at
    /// 328 ms for one node against a 20,000-row store, identical for a
    /// *missing* id. See `tests/query_plan_tests.rs`.
    fn get_node_projecting(&self, id: &str, query: &str) -> Result<Option<Node>, KbStoreError> {
        let result = self
            .run_immut_params(query, btree_params([("id", dv_str(id))]))
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
}

impl KbStore for CozoKbStore {
    fn insert_node(&self, node: &Node) -> Result<(), KbStoreError> {
        self.run_mut_params(Self::NODE_PUT_SCRIPT, self.node_put_params(node)?)
            .map_err(cozo_err)?;
        self.update_links_for_node(node)?;
        // ADR-065 item 4: same write-path-independence fix as the typed-link
        // call above, for the sibling `#+TRANSCLUDE:` directive — see
        // `update_meta_members_for_node`'s doc comment.
        self.update_meta_members_for_node(node)?;
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
            "?[src, dst, rel_type] := src = $id, *links{src, dst, rel_type}\n:rm links {src, dst, rel_type}",
            btree_params([("id", dv_str(id))]),
        )
        .map_err(cozo_err)?;

        Ok(())
    }

    fn get_node(&self, id: &str) -> Result<Option<Node>, KbStoreError> {
        self.get_node_projecting(
            id,
            r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                    aliases_json, properties_json, crdt_doc, has_crdt]
                    := id = $id,
                       *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                              aliases_json, properties_json, crdt_doc, has_crdt}"#,
        )
    }

    fn get_node_light(&self, id: &str) -> Result<Option<Node>, KbStoreError> {
        // Same column ORDER as `get_node`, stopping before `crdt_doc`, so the
        // shared `row_to_node` decoder works unchanged: it reads `has_crdt`
        // from index 12 and the blob from index 11, both of which are simply
        // absent here, yielding `has_crdt = false` / `crdt_doc = None`.
        self.get_node_projecting(
            id,
            r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                    aliases_json, properties_json]
                    := id = $id,
                       *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                              aliases_json, properties_json}"#,
        )
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
        // matches (defensive against stale FTS index entries). Bulk-fetch
        // title+body for all candidates in ONE query, then verify in Rust,
        // preserving the FTS score order.
        //
        // @ai-caution: [kb-query] The candidate ids drive a temp relation that
        // is JOINED against `nodes` on its primary key. Two shapes that look
        // simpler are both wrong:
        //
        //  - `*nodes{id, title, body}, is_in(id, $ids)` — the previous form,
        //    and the single most expensive query in the KB. `is_in` is
        //    `right.contains(left)` (`cozo-0.7.6/src/data/functions.rs`), a
        //    linear `Vec` probe, and the relation atom is unbound, so cozo
        //    scans every row and probes the candidate list once per row:
        //    20,000 x 70 string comparisons to fetch 70 rows whose primary
        //    keys were already in hand. Measured at ~113 ms of a 123 ms
        //    search.
        //  - collapsing this into the FTS query above as
        //    `~nodes:fts{id, title, body | ...}` — tempting, because
        //    `SearchInput::normalize_fts` does bind any base-relation column
        //    named in the search head, and it costs no extra I/O. But it
        //    destroys this guard on the exact case the guard exists for. When
        //    an index posting outlives its base row, cozo's FTS operator does
        //    `base_handle.get(...)` itself: on sqlite that is
        //    `ok_or("corrupted index")`, so the WHOLE search fails; on sled the
        //    ghost row yields a SHORT tuple and `bind_score` lands in the
        //    `title` position, silently mis-assigning every projected column.
        //    Verified both, by deleting a base row through
        //    `DbInstance::import_relations`, which (unlike `:rm`) maintains
        //    regular indices but never `fts_indices`. The join below has
        //    neither failure mode: a candidate with no base row simply
        //    produces no join row and is dropped, which is the documented
        //    intent.
        let candidate_rows: Vec<DataValue> = result
            .rows
            .iter()
            .filter_map(|row| {
                row.first()
                    .and_then(|v| v.get_str())
                    .map(|id| DataValue::List(vec![dv_str(id)]))
            })
            .collect();

        let mut content: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::with_capacity(candidate_rows.len());
        if !candidate_rows.is_empty() {
            let fetched = self
                .run_immut_params(
                    "cand[id] <- $ids\n?[id, title, body] := cand[id], *nodes{id, title, body}",
                    btree_params([("ids", DataValue::List(candidate_rows))]),
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

        // Verification terms must be tokenized the SAME way the index tokenized
        // the document, or this guard silently deletes correct results.
        //
        // @ai-caution: [kb-search] Do NOT revert this to
        // `query.to_lowercase().split_whitespace()`. That treats FTS query
        // syntax as literal text: the prefix query `buffer*` tokenizes to the
        // single term `buffer*`, which no document text ever `contains`, so
        // EVERY candidate the index correctly returned was dropped and the
        // caller saw zero hits (verified: `buffer*` matched 1 row in
        // `nodes:fts` and `fts_search` returned 0). Same for any query carrying
        // `:`/`-`/`*` that cozo's parser does accept. Splitting on
        // `!is_alphanumeric` mirrors cozo's `Simple` tokenizer, so the guard
        // now checks exactly what was indexed.
        let query_lower = query.to_lowercase();
        let query_terms: Vec<&str> = query_lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        let mut hits = Vec::new();
        for row in &result.rows {
            let Some(id) = row.first().and_then(|v| v.get_str()) else {
                continue;
            };
            let score = row.get(1).and_then(|v| v.get_float()).unwrap_or(0.0);
            // A candidate with no row in `nodes` is a stale index entry — drop
            // it, which is what this guard exists for. A candidate whose row IS
            // present passes when it still contains a query term; a query with
            // no alphanumeric content leaves nothing to check, so trust the
            // index rather than vetoing every row.
            //
            // Scope correction (2026-08, verified rather than assumed): this
            // branch does NOT catch every stale posting, and the comment above
            // used to imply it did. Cozo dereferences the base row inside the
            // FTS operator, so on the sqlite backend a posting whose base row
            // was deleted aborts the query with "corrupted index" before
            // control ever reaches here. The branch is reachable on sled, where
            // `:rm` leaves a ghost row, and for any candidate the base relation
            // legitimately no longer holds. A genuine index/base divergence on
            // sqlite therefore surfaces as a hard `KbStoreError`, not a
            // degraded result set — tracked separately; do not paper over it
            // here.
            let Some((title, body)) = content.get(id) else {
                continue;
            };
            let verified = query_terms.is_empty() || {
                let text = format!("{title} {body}").to_lowercase();
                query_terms.iter().any(|term| text.contains(term))
            };
            if verified {
                hits.push(SearchHit {
                    id: id.to_string(),
                    score,
                });
                if hits.len() >= limit {
                    break;
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
            ?[src, dst, rel_type] := src = $src, dst = $dst, *links{src, dst, rel_type}
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
                "?[src, dst, rel_type, display, weight, confidence] := src = $id, *links{src, dst, rel_type, display, weight, confidence}",
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| parse_link_row(row))
            .collect())
    }

    /// @ai-caution: [kb-query] This is the ONE query in this module that
    /// genuinely cannot be turned into a prefix seek by pre-binding, and it is
    /// left in the post-filter form deliberately. `links` is keyed
    /// `(src, dst, rel_type)`; `dst` sits at position 1, so binding it joins
    /// position {1}, which `cozo-0.7.6/src/query/ra.rs:1509 join_is_prefix`
    /// rejects (it requires exactly `0..n`) — the plan degrades to
    /// `stored_mat_join`, which materialises the whole relation, i.e. the same
    /// scan with extra allocation. Backlinks need a secondary index keyed on
    /// `dst` (`links:by_dst`), tracked in #265 and #753; do not "fix" this by
    /// moving the equality, which would make it slower, not faster.
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
                // @ai-caution: [data-loss] Do NOT restore the old behaviour of
                // returning `Ok(Vec::new())` here. This branch is not rare and
                // it is not "a store that needs repair": `:rm nodes {id}` leaves
                // a 1-length tombstone tuple on the sled backend, so the
                // 13-column bind above fails with "tuple bound by variable
                // 'title' is too short" after ANY node deletion. Returning empty
                // meant the entire KB read as gone — every node, not just the
                // deleted one — while `get_node` still resolved each of them
                // individually. `kb_build.rs` already worked around the same
                // tombstone locally ("`:rm` leaves partial tuples that break
                // load_all()") without fixing it here.
                //
                // Fall back to a per-id load instead. Slower, but it returns the
                // nodes that are actually there, and skips only the tombstones.
                tracing::warn!(error = %e, "KB store: bulk node load failed (likely deletion tombstones) — falling back to per-id load");
                return self.load_all_per_id();
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

    fn replace_all_nodes(&self, nodes: &[&Node]) -> Result<(), KbStoreError> {
        // Clear existing data by delegating to `delete_node`, which is the one
        // deletion path in this file that is known-correct.
        //
        // @ai-caution: [data-loss] Do NOT re-inline a bulk `:rm` here. The
        // previous form was `?[id] := *nodes{id}` + `:rm nodes {id}` — a
        // self-referential read-write in one transaction. It did not remove the
        // rows; it truncated each to its key, leaving `["n1"]`-shaped
        // short-arity remnants with every non-key column destroyed in place.
        // `load_all` then could not read the relation and degraded to an empty
        // result (its B-5 tolerance), so the store reported EMPTY while the ids
        // were still on disk. `delete_node` materialises the row set first
        // (`?[id] <- [[$id]]`), which is why it has always been correct — and
        // reusing it means there is one deletion implementation, not three
        // (principle #8). The legitimate caller (`migrate.rs`) targets a fresh
        // store, so this loop is a no-op there.
        let existing_ids: Vec<String> = {
            let (_cols, rows) = self.raw_query(r#"?[id] := *nodes{id}"#)?;
            rows.into_iter()
                .filter_map(|r| r.into_iter().next())
                .map(|id| id.trim_matches('"').to_string())
                .collect()
        };
        for id in &existing_ids {
            self.delete_node(id)?;
        }

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

    fn store_embedding(
        &self,
        id: &str,
        model: &str,
        content_hash: &str,
        vec: &[f32],
    ) -> Result<(), KbStoreError> {
        CozoKbStore::store_embedding(self, id, model, content_hash, vec)
    }

    fn vector_search(
        &self,
        model: &str,
        vec: &[f32],
        k: usize,
    ) -> Result<Vec<VectorHit>, KbStoreError> {
        CozoKbStore::vector_search_for_model(self, model, vec, k)
    }

    fn graphrag_search(
        &self,
        model: &str,
        vec: &[f32],
        k: usize,
    ) -> Result<Vec<VectorHit>, KbStoreError> {
        CozoKbStore::graphrag_search(self, model, vec, k)
    }

    fn get_cached_embedding(
        &self,
        content_hash: &str,
        model: &str,
        chunk_version: i64,
    ) -> Result<Option<Vec<f32>>, KbStoreError> {
        CozoKbStore::get_cached_embedding(self, content_hash, model, chunk_version)
    }

    fn put_cached_embedding(
        &self,
        content_hash: &str,
        model: &str,
        chunk_version: i64,
        vec: &[f32],
    ) -> Result<(), KbStoreError> {
        CozoKbStore::put_cached_embedding(self, content_hash, model, chunk_version, vec)
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
}
