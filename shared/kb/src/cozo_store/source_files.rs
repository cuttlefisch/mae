//! Source-file tracking for the ingestion pipeline: persisting an
//! in-memory `KnowledgeBase` into this store, recording/looking up a
//! file's content hash + produced node ids (with retraction of ids the
//! file no longer produces), and removing/listing tracked files.

use super::util::{btree_params, cozo_err, dv_str};
use super::*;
use std::collections::HashMap;

impl CozoKbStore {
    /// Persist all nodes from an in-memory `KnowledgeBase` into this CozoDB store.
    ///
    /// Used by `build-manual-kb` to create the pre-built manual KB file.
    /// Returns the number of nodes persisted.
    pub fn persist_nodes(&self, kb: &crate::KnowledgeBase) -> Result<usize, KbStoreError> {
        let mut count = 0;
        for node in kb.nodes_values() {
            self.insert_node(node)?;
            count += 1;
        }
        Ok(count)
    }
    /// Record a source file's metadata for incremental ingestion.
    ///
    /// Retracts any id this file previously produced but no longer does (e.g.
    /// an in-place `:ID:` edit) — without this, a full `:kb-reimport` never
    /// self-heals a renamed id, since re-walking the directory only ever
    /// upserts whatever the file currently contains and this function's
    /// deletion-detection (`federation.rs`) only fires on whole-file removal.
    pub fn record_source_file(
        &self,
        file_path: &str,
        content_hash: &str,
        mtime: i64,
        node_ids: &[String],
    ) -> Result<(), KbStoreError> {
        let prev_ids = self.get_source_file_node_ids(file_path)?;
        for old_id in prev_ids.iter().filter(|id| !node_ids.contains(id)) {
            self.delete_node(old_id)?;
        }
        let node_ids_json =
            serde_json::to_string(node_ids).map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let now = self.now_epoch();
        self.run_mut_params(
            r#"?[file_path, content_hash, last_mtime, node_ids_json, last_import] <- [[
                $file_path, $content_hash, $last_mtime, $node_ids_json, $now
            ]]
            :put source_files {
                file_path => content_hash, last_mtime, node_ids_json, last_import
            }"#,
            btree_params([
                ("file_path", dv_str(file_path)),
                ("content_hash", dv_str(content_hash)),
                ("last_mtime", DataValue::from(mtime)),
                ("node_ids_json", dv_str(&node_ids_json)),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }
    /// Get a source file's stored content hash (for change detection).
    pub fn get_source_file_hash(&self, file_path: &str) -> Result<Option<String>, KbStoreError> {
        let result = self
            .run_immut_params(
                r#"?[content_hash] := *source_files{file_path: $fp, content_hash}"#,
                btree_params([("fp", dv_str(file_path))]),
            )
            .map_err(cozo_err)?;
        Ok(result
            .rows
            .first()
            .and_then(|r| r[0].get_str())
            .map(|s| s.to_string()))
    }
    /// Get node IDs associated with a source file (for deletion on file removal).
    pub fn get_source_file_node_ids(&self, file_path: &str) -> Result<Vec<String>, KbStoreError> {
        let result = self
            .run_immut_params(
                r#"?[node_ids_json] := *source_files{file_path: $fp, node_ids_json}"#,
                btree_params([("fp", dv_str(file_path))]),
            )
            .map_err(cozo_err)?;
        if let Some(row) = result.rows.first() {
            if let Some(json) = row[0].get_str() {
                let ids: Vec<String> = serde_json::from_str(json).unwrap_or_default();
                return Ok(ids);
            }
        }
        Ok(Vec::new())
    }
    /// Remove a source file record and its associated nodes.
    pub fn remove_source_file(&self, file_path: &str) -> Result<Vec<String>, KbStoreError> {
        let node_ids = self.get_source_file_node_ids(file_path)?;
        for id in &node_ids {
            self.delete_node(id)?;
        }
        self.run_mut_params(
            r#"?[file_path] <- [[$fp]]
            :rm source_files {file_path}"#,
            btree_params([("fp", dv_str(file_path))]),
        )
        .map_err(cozo_err)?;
        Ok(node_ids)
    }
    /// Build a reverse index (node id -> source file path) from the
    /// `source_files` relation.
    ///
    /// The `nodes` relation itself has no `source_file` column — only this
    /// file->node_ids mapping persists it, written by `record_source_file`
    /// during ingest. `load_all` uses this to reconstruct each `Node`'s
    /// `source_file` at load time; without it, `source_file` is `None` for
    /// every node loaded from a fresh store open (correct only transiently,
    /// in the same process that just ingested it), and `kb_node_source_file`
    /// reports "No source file" for nodes whose backing file plainly exists
    /// on disk. See #323-adjacent investigation, 2026-07-13.
    pub fn source_file_by_node_id(&self) -> Result<HashMap<String, PathBuf>, KbStoreError> {
        let result = self
            .run_immut(r#"?[file_path, node_ids_json] := *source_files{file_path, node_ids_json}"#)
            .map_err(cozo_err)?;
        let mut map = HashMap::new();
        for row in &result.rows {
            let Some(file_path) = row.first().and_then(|v| v.get_str()) else {
                continue;
            };
            let Some(json) = row.get(1).and_then(|v| v.get_str()) else {
                continue;
            };
            let ids: Vec<String> = serde_json::from_str(json).unwrap_or_default();
            let path = PathBuf::from(file_path);
            for id in ids {
                map.insert(id, path.clone());
            }
        }
        Ok(map)
    }
    /// List all tracked source files with their content hashes.
    pub fn list_source_files(&self) -> Result<Vec<(String, String, i64)>, KbStoreError> {
        let result = self
            .run_immut(
                r#"?[file_path, content_hash, last_mtime]
                   := *source_files{file_path, content_hash, last_mtime}
                   :order file_path"#,
            )
            .map_err(cozo_err)?;
        let mut files = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            let fp = row[0].get_str().unwrap_or("").to_string();
            let hash = row[1].get_str().unwrap_or("").to_string();
            let mtime = row.get(2).and_then(|v| v.get_int()).unwrap_or(0);
            files.push((fp, hash, mtime));
        }
        Ok(files)
    }
}
