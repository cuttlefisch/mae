//! Node versioning (Phase H): append-only snapshots with content-hash
//! integrity checks, version history, and checksum-verified restore.

use super::util::{btree_params, cozo_err, dv_str};
use super::*;

impl CozoKbStore {
    /// Snapshot the current state of a node into node_versions.
    /// Computes a content checksum for tamper detection (SOC II audit trail).
    pub fn snapshot_version(&self, id: &str, change_summary: &str) -> Result<i64, KbStoreError> {
        // Get current max version for this node
        let ver_result = self
            .run_immut_params(
                "?[v] := *node_versions{id, version: v}, id = $id :order -v :limit 1",
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;
        let next_version = ver_result
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0)
            + 1;

        // Read current node state
        let node = self
            .get_node(id)?
            .ok_or_else(|| KbStoreError::NotFound(id.to_string()))?;

        let tags_json = serde_json::to_string(&node.tags).unwrap_or_else(|_| "[]".to_string());
        let properties_json =
            serde_json::to_string(&node.properties).unwrap_or_else(|_| "{}".to_string());
        let todo_state_str = node.todo_state.as_deref().unwrap_or("");
        let priority_str = node.priority.map(|c| c.to_string()).unwrap_or_default();
        let content_hash = NodeVersion::compute_hash(
            &node.title,
            &node.body,
            &tags_json,
            todo_state_str,
            &priority_str,
        );
        let now = self.now_epoch();

        self.run_mut_params(
            r#"?[id, version, title, body, tags_json, todo_state, priority, properties_json, assignee, change_summary, author, content_hash, created_at] <- [[
                $id, $version, $title, $body, $tags_json, $todo_state, $priority, $properties_json, "", $summary, "local", $hash, $now
            ]]
            :put node_versions {id, version => title, body, tags_json, todo_state, priority, properties_json, assignee, change_summary, author, content_hash, created_at}"#,
            btree_params([
                ("id", dv_str(id)),
                ("version", DataValue::from(next_version)),
                ("title", dv_str(&node.title)),
                ("body", dv_str(&node.body)),
                ("tags_json", dv_str(&tags_json)),
                ("todo_state", dv_str(todo_state_str)),
                ("priority", dv_str(&priority_str)),
                ("properties_json", dv_str(&properties_json)),
                ("summary", dv_str(change_summary)),
                ("hash", dv_str(&content_hash)),
                ("now", DataValue::from(now)),
            ]),
        ).map_err(cozo_err)?;

        Ok(next_version)
    }
    /// Get version history for a node (newest first).
    pub fn node_history(&self, id: &str, limit: usize) -> Result<Vec<NodeVersion>, KbStoreError> {
        let result = self.run_immut_params(
            &format!(
                "?[version, title, body, tags_json, todo_state, priority, properties_json, change_summary, author, content_hash, created_at] := *node_versions{{id, version, title, body, tags_json, todo_state, priority, properties_json, change_summary, author, content_hash, created_at}}, id = $id :order -version :limit {limit}"
            ),
            btree_params([("id", dv_str(id))]),
        ).map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                Some(NodeVersion {
                    version: row.first()?.get_int()?,
                    title: row.get(1)?.get_str()?.to_string(),
                    body: row.get(2)?.get_str()?.to_string(),
                    tags_json: row.get(3)?.get_str()?.to_string(),
                    todo_state: row.get(4)?.get_str()?.to_string(),
                    priority: row.get(5)?.get_str()?.to_string(),
                    change_summary: row.get(7)?.get_str()?.to_string(),
                    author: row.get(8)?.get_str()?.to_string(),
                    content_hash: row.get(9)?.get_str()?.to_string(),
                    created_at: row.get(10)?.get_int()?,
                })
            })
            .collect())
    }
    /// Restore a node to a specific version.
    ///
    /// Verifies the content hash before applying to detect tampered versions.
    /// Returns `KbStoreError::Storage` if integrity check fails.
    pub fn restore_version(&self, id: &str, version: i64) -> Result<(), KbStoreError> {
        let result = self.run_immut_params(
            "?[title, body, tags_json, todo_state, priority, content_hash] := *node_versions{id, version, title, body, tags_json, todo_state, priority, content_hash}, id = $id, version = $version",
            btree_params([
                ("id", dv_str(id)),
                ("version", DataValue::from(version)),
            ]),
        ).map_err(cozo_err)?;

        let row = result
            .rows
            .first()
            .ok_or_else(|| KbStoreError::NotFound(format!("{id}@v{version}")))?;

        let title = row
            .first()
            .and_then(|v| v.get_str())
            .unwrap_or("")
            .to_string();
        let body = row
            .get(1)
            .and_then(|v| v.get_str())
            .unwrap_or("")
            .to_string();
        let tags_json = row.get(2).and_then(|v| v.get_str()).unwrap_or("[]");
        let todo_state_str = row.get(3).and_then(|v| v.get_str()).unwrap_or("");
        let priority_str = row.get(4).and_then(|v| v.get_str()).unwrap_or("");
        let stored_hash = row.get(5).and_then(|v| v.get_str()).unwrap_or("");

        // Integrity check: verify content hash before restoring
        let computed_hash =
            NodeVersion::compute_hash(&title, &body, tags_json, todo_state_str, priority_str);
        if !stored_hash.is_empty() && stored_hash != computed_hash {
            return Err(KbStoreError::Storage(format!(
                "integrity check failed for {id}@v{version}: stored hash {stored_hash} != computed {computed_hash}"
            )));
        }

        let tags: Vec<String> = serde_json::from_str(tags_json).unwrap_or_default();
        let todo_state = if todo_state_str.is_empty() {
            None
        } else {
            Some(todo_state_str.to_string())
        };
        let priority = priority_str.chars().next();

        // Snapshot current state before restore
        self.snapshot_version(id, &format!("pre-restore to v{version}"))?;

        // Get current node to preserve non-versioned fields
        let mut node = self
            .get_node(id)?
            .ok_or_else(|| KbStoreError::NotFound(id.to_string()))?;
        node.title = title;
        node.body = body;
        node.tags = tags;
        node.todo_state = todo_state;
        node.priority = priority;

        self.update_node(&node)?;

        // Snapshot the restored state
        self.snapshot_version(id, &format!("restored to v{version}"))?;

        Ok(())
    }
}
