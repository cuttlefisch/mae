//! The `node_attachments` relation: each node's `attachments` map (ADR-115
//! D4) as rows `(node_id, name) => sha256, size, media_type`.
//!
//! A projection like `links`: written with the node row in the same
//! transaction (`write_node_atomically`), removed with it (`delete_node`), and
//! read back onto `Node::attachments` by every full loader, so a load, edit
//! and store of a node never drops its map. The store is one KB's, so the KB
//! is the store and is not a column.
//!
//! It answers "which nodes of this KB reference which blobs" for queries. It
//! is NOT the sweep's live set (ADR-115 D5): that is derived from CRDT truth
//! and every history version, which a projection that can drift must not
//! stand in for.

use std::collections::{BTreeMap, HashMap};

use super::util::{btree_params, cozo_err, dv_str};
use super::*;
use crate::NodeAttachment;

impl CozoKbStore {
    pub(super) const ATTACHMENTS_DDL: &'static str = r#":create node_attachments {
                node_id: String,
                name: String
                =>
                sha256: String,
                size: Int,
                media_type: String
            }"#;
    /// Remove every attachment row of node `$id`.
    pub(super) const ATTACHMENTS_RM_SCRIPT: &'static str = r#"?[node_id, name] := node_id = $id, *node_attachments{node_id, name}
            :rm node_attachments {node_id, name}"#;
    /// Insert attachment rows `[node_id, name, sha256, size, media_type]` from `$rows`.
    pub(super) const ATTACHMENTS_BULK_SCRIPT: &'static str = r#"?[node_id, name, sha256, size, media_type] <- $rows
            :put node_attachments {node_id, name => sha256, size, media_type}"#;

    /// The rows `node`'s map projects to, for [`Self::ATTACHMENTS_BULK_SCRIPT`].
    pub(super) fn attachment_rows(node: &Node) -> Vec<DataValue> {
        node.attachments
            .iter()
            .map(|(name, a)| {
                DataValue::List(vec![
                    dv_str(&node.id),
                    dv_str(name),
                    dv_str(&a.sha256),
                    // Exact: the reader bounds a size at 2^53 - 1.
                    DataValue::from(a.size as i64),
                    dv_str(&a.media_type),
                ])
            })
            .collect()
    }

    fn row_to_attachment(row: &[DataValue]) -> Option<(String, String, NodeAttachment)> {
        let s = |i: usize| row.get(i).and_then(|v| v.get_str()).map(str::to_string);
        Some((
            s(0)?,
            s(1)?,
            NodeAttachment {
                sha256: s(2)?,
                size: u64::try_from(row.get(3)?.get_int()?).ok()?,
                media_type: s(4)?,
            },
        ))
    }

    /// Node `id`'s attachments as stored.
    pub(super) fn attachments_of(
        &self,
        id: &str,
    ) -> Result<BTreeMap<String, NodeAttachment>, KbStoreError> {
        let rows = self
            .run_immut_params(
                "?[node_id, name, sha256, size, media_type] := node_id = $id, \
                 *node_attachments{node_id, name, sha256, size, media_type}",
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;
        Ok(rows
            .rows
            .iter()
            .filter_map(|r| Self::row_to_attachment(r))
            .map(|(_, name, a)| (name, a))
            .collect())
    }

    /// Every node's attachments as stored, by node id: one query for a bulk load.
    pub(super) fn attachments_by_node(
        &self,
    ) -> Result<HashMap<String, BTreeMap<String, NodeAttachment>>, KbStoreError> {
        let rows = self
            .run_immut(
                "?[node_id, name, sha256, size, media_type] := \
                 *node_attachments{node_id, name, sha256, size, media_type}",
            )
            .map_err(cozo_err)?;
        let mut out: HashMap<String, BTreeMap<String, NodeAttachment>> = HashMap::new();
        for (node, name, a) in rows.rows.iter().filter_map(|r| Self::row_to_attachment(r)) {
            out.entry(node).or_default().insert(name, a);
        }
        Ok(out)
    }

    /// Fill each of `nodes`' map from the relation, for a bulk load. An error
    /// fails the load rather than handing back nodes whose next store would
    /// delete their rows.
    pub(super) fn load_attachments_into(&self, nodes: &mut [Node]) -> Result<(), KbStoreError> {
        let mut by_node = self.attachments_by_node()?;
        for node in nodes {
            if let Some(map) = by_node.remove(&node.id) {
                node.attachments = map;
            }
        }
        Ok(())
    }

    /// Every `(node_id, name, sha256)` row, sorted: which nodes of this KB
    /// reference which blobs, under which names.
    pub fn attachment_refs(&self) -> Result<Vec<(String, String, String)>, KbStoreError> {
        let mut out: Vec<(String, String, String)> = self
            .attachments_by_node()?
            .into_iter()
            .flat_map(|(node, map)| {
                map.into_iter()
                    .map(move |(name, a)| (node.clone(), name, a.sha256))
            })
            .collect();
        out.sort();
        Ok(out)
    }
}
