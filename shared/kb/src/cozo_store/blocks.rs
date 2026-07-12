//! Meta-node composition + block-level addressing (Phase D): ordered
//! member references on a meta-node, composing a meta-node's body from
//! its members, and splitting/reading a node body's paragraph blocks.

use super::util::{btree_params, cozo_err, dv_str};
use super::*;

impl CozoKbStore {
    /// Get ordered members of a meta-node.
    pub fn meta_members(&self, meta_id: &str) -> Result<Vec<MetaMember>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[member_id, position, role] := *meta_members{meta_id, member_id, position, role}, meta_id = $id :order position",
                btree_params([("id", dv_str(meta_id))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                Some(MetaMember {
                    member_id: row.first()?.get_str()?.to_string(),
                    position: row.get(1)?.get_int()? as i32,
                    role: row.get(2)?.get_str()?.to_string(),
                })
            })
            .collect())
    }
    /// Add a member to a meta-node.
    pub fn add_meta_member(
        &self,
        meta_id: &str,
        member_id: &str,
        position: i32,
        role: &str,
    ) -> Result<(), KbStoreError> {
        self.run_mut_params(
            r#"?[meta_id, member_id, position, role] <- [[$meta_id, $member_id, $position, $role]]
            :put meta_members {meta_id, member_id, position => role}"#,
            btree_params([
                ("meta_id", dv_str(meta_id)),
                ("member_id", dv_str(member_id)),
                ("position", DataValue::from(position as i64)),
                ("role", dv_str(role)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }
    /// Remove a member from a meta-node.
    pub fn remove_meta_member(&self, meta_id: &str, member_id: &str) -> Result<(), KbStoreError> {
        self.run_mut_params(
            r#"?[meta_id, member_id, position] := *meta_members{meta_id, member_id, position}, meta_id = $meta_id, member_id = $member_id
            :rm meta_members {meta_id, member_id, position}"#,
            btree_params([
                ("meta_id", dv_str(meta_id)),
                ("member_id", dv_str(member_id)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }
    /// Compose a meta-node's body from its members.
    pub fn compose_meta_body(&self, meta_id: &str) -> Result<String, KbStoreError> {
        let members = self.meta_members(meta_id)?;
        let mut parts = Vec::new();
        for member in &members {
            match member.role.as_str() {
                "content" | "transclusion" => {
                    if let Ok(Some(node)) = self.get_node(&member.member_id) {
                        parts.push(node.body);
                    }
                }
                "reference" => {
                    parts.push(format!("→ [[{}]]", member.member_id));
                }
                _ => {}
            }
        }
        Ok(parts.join("\n\n"))
    }
    /// Split a node body into paragraph blocks and store them.
    pub fn split_into_blocks(&self, parent_id: &str) -> Result<usize, KbStoreError> {
        let node = self
            .get_node(parent_id)?
            .ok_or_else(|| KbStoreError::NotFound(parent_id.to_string()))?;

        let now = self.now_epoch();
        // Remove existing blocks
        self.run_mut_params(
            "?[parent_id, block_idx] := *blocks{parent_id, block_idx}, parent_id = $id\n:rm blocks {parent_id, block_idx}",
            btree_params([("id", dv_str(parent_id))]),
        )
        .map_err(cozo_err)?;

        let paragraphs: Vec<&str> = node.body.split("\n\n").collect();
        for (idx, content) in paragraphs.iter().enumerate() {
            let block_type = if content.starts_with('#') || content.starts_with('*') {
                "heading"
            } else if content.starts_with("```") || content.starts_with("#+begin_src") {
                "code"
            } else if content.starts_with("- ") || content.starts_with("1.") {
                "list"
            } else {
                "paragraph"
            };
            self.run_mut_params(
                r#"?[parent_id, block_idx, content, block_type, created_at, updated_at] <- [[$pid, $idx, $content, $btype, $now, $now]]
                :put blocks {parent_id, block_idx => content, block_type, created_at, updated_at}"#,
                btree_params([
                    ("pid", dv_str(parent_id)),
                    ("idx", DataValue::from(idx as i64)),
                    ("content", dv_str(content)),
                    ("btype", dv_str(block_type)),
                    ("now", DataValue::from(now)),
                ]),
            )
            .map_err(cozo_err)?;
        }
        Ok(paragraphs.len())
    }
    /// Get all blocks for a node.
    pub fn get_blocks(&self, parent_id: &str) -> Result<Vec<Block>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[block_idx, content, block_type] := *blocks{parent_id, block_idx, content, block_type}, parent_id = $id :order block_idx",
                btree_params([("id", dv_str(parent_id))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                Some(Block {
                    block_idx: row.first()?.get_int()? as usize,
                    content: row.get(1)?.get_str()?.to_string(),
                    block_type: row.get(2)?.get_str()?.to_string(),
                })
            })
            .collect())
    }
    /// Get a single block by index.
    pub fn get_block(&self, parent_id: &str, idx: usize) -> Result<Option<Block>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[block_idx, content, block_type] := *blocks{parent_id, block_idx, content, block_type}, parent_id = $id, block_idx = $idx",
                btree_params([
                    ("id", dv_str(parent_id)),
                    ("idx", DataValue::from(idx as i64)),
                ]),
            )
            .map_err(cozo_err)?;

        Ok(result.rows.first().and_then(|row| {
            Some(Block {
                block_idx: row.first()?.get_int()? as usize,
                content: row.get(1)?.get_str()?.to_string(),
                block_type: row.get(2)?.get_str()?.to_string(),
            })
        }))
    }
}
