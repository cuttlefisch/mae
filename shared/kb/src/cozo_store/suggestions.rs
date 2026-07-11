//! AI hygiene-suggestion tracking: insert/list/update-status/dedupe/clear
//! over the `hygiene_suggestions` relation.

use super::util::{btree_params, cozo_err, dv_str};
use super::*;

impl CozoKbStore {
    /// Insert a hygiene suggestion. Returns the suggestion_id assigned.
    pub fn insert_suggestion(
        &self,
        node_id: &str,
        category: &str,
        message: &str,
        action_json: &str,
        confidence: f64,
    ) -> Result<i64, KbStoreError> {
        // Get next suggestion_id for this node
        let max_id = self
            .run_immut_params(
                "?[m] := *hygiene_suggestions{node_id: $nid, suggestion_id: sid}, m = max(sid)\n\
                 ?[m] := m = 0, not *hygiene_suggestions{node_id: $nid}",
                btree_params([("nid", dv_str(node_id))]),
            )
            .map_err(cozo_err)?;
        let next_id = max_id
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0)
            + 1;

        let now = self.now_epoch();
        self.run_mut_params(
            "?[node_id, suggestion_id, category, message, suggested_action_json, confidence, status, created_at] <- \
             [[$nid, $sid, $cat, $msg, $action, $conf, 'pending', $now]] \
             :put hygiene_suggestions { node_id, suggestion_id => category, message, suggested_action_json, confidence, status, created_at }",
            btree_params([
                ("nid", dv_str(node_id)),
                ("sid", DataValue::from(next_id)),
                ("cat", dv_str(category)),
                ("msg", dv_str(message)),
                ("action", dv_str(action_json)),
                ("conf", DataValue::from(confidence)),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(next_id)
    }
    /// List pending suggestions, optionally filtered by category.
    pub fn list_suggestions(
        &self,
        category: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<crate::hygiene::HygieneSuggestion>, KbStoreError> {
        use crate::hygiene::HygieneSuggestion;

        let status_filter = status.unwrap_or("pending");
        let (query, params) = if let Some(cat) = category {
            (
                "?[node_id, suggestion_id, category, message, suggested_action_json, confidence, status, created_at] := \
                 *hygiene_suggestions{node_id, suggestion_id, category, message, suggested_action_json, confidence, status, created_at}, \
                 status == $status, category == $cat",
                btree_params([
                    ("status", dv_str(status_filter)),
                    ("cat", dv_str(cat)),
                ]),
            )
        } else {
            (
                "?[node_id, suggestion_id, category, message, suggested_action_json, confidence, status, created_at] := \
                 *hygiene_suggestions{node_id, suggestion_id, category, message, suggested_action_json, confidence, status, created_at}, \
                 status == $status",
                btree_params([("status", dv_str(status_filter))]),
            )
        };

        let result = self.run_immut_params(query, params).map_err(cozo_err)?;
        let mut suggestions = Vec::new();
        for row in result.rows.iter() {
            if let (
                Some(nid),
                Some(sid),
                Some(cat),
                Some(msg),
                Some(action),
                Some(conf),
                Some(st),
                Some(ts),
            ) = (
                row.first().and_then(|v| v.get_str()),
                row.get(1).and_then(|v| v.get_int()),
                row.get(2).and_then(|v| v.get_str()),
                row.get(3).and_then(|v| v.get_str()),
                row.get(4).and_then(|v| v.get_str()),
                row.get(5).and_then(|v| v.get_float()),
                row.get(6).and_then(|v| v.get_str()),
                row.get(7).and_then(|v| v.get_int()),
            ) {
                suggestions.push(HygieneSuggestion {
                    node_id: nid.to_string(),
                    suggestion_id: sid,
                    category: cat.to_string(),
                    message: msg.to_string(),
                    suggested_action_json: action.to_string(),
                    confidence: conf,
                    status: st.to_string(),
                    created_at: ts,
                });
            }
        }
        Ok(suggestions)
    }
    /// Update a suggestion's status (accept or dismiss).
    pub fn update_suggestion_status(
        &self,
        node_id: &str,
        suggestion_id: i64,
        new_status: &str,
    ) -> Result<(), KbStoreError> {
        self.run_mut_params(
            "?[node_id, suggestion_id, status] <- [[$nid, $sid, $status]] \
             :update hygiene_suggestions { node_id, suggestion_id => status }",
            btree_params([
                ("nid", dv_str(node_id)),
                ("sid", DataValue::from(suggestion_id)),
                ("status", dv_str(new_status)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }
    /// Check if a suggestion already exists for the given node+category (any status).
    pub fn has_suggestion(&self, node_id: &str, category: &str) -> Result<bool, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[node_id] := *hygiene_suggestions{node_id: $nid, category: $cat, node_id}",
                btree_params([("nid", dv_str(node_id)), ("cat", dv_str(category))]),
            )
            .map_err(cozo_err)?;
        Ok(!result.rows.is_empty())
    }
    /// Delete all suggestions for a given node (e.g., after the node is fixed).
    pub fn clear_suggestions_for_node(&self, node_id: &str) -> Result<(), KbStoreError> {
        self.run_mut_params(
            "?[node_id, suggestion_id] := *hygiene_suggestions{node_id, suggestion_id}, node_id == $nid \
             :rm hygiene_suggestions { node_id, suggestion_id }",
            btree_params([("nid", dv_str(node_id))]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }
}
