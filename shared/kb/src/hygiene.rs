//! KB hygiene suggestion types and store methods.
//!
//! Hygiene suggestions are non-destructive recommendations for improving
//! KB quality. Categories include: orphan nodes, broken links, kind
//! mismatches, missing metadata, and (future) AI-inferred link types.

use serde::{Deserialize, Serialize};

/// A hygiene suggestion for a KB node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HygieneSuggestion {
    /// The node this suggestion applies to.
    pub node_id: String,
    /// Unique suggestion ID (auto-incremented).
    pub suggestion_id: i64,
    /// Category: orphan, broken_link, kind_mismatch, missing_metadata,
    /// missing_link, wrong_link_type.
    pub category: String,
    /// Human-readable description.
    pub message: String,
    /// JSON-encoded suggested action (add_link, change_link_type, set_field).
    pub suggested_action_json: String,
    /// Confidence score (0.0–1.0). Deterministic checks use 1.0.
    pub confidence: f64,
    /// Status: "pending", "accepted", "dismissed".
    pub status: String,
    /// Unix timestamp when created.
    pub created_at: i64,
}

/// Category constants.
pub const CAT_ORPHAN: &str = "orphan";
pub const CAT_BROKEN_LINK: &str = "broken_link";
pub const CAT_KIND_MISMATCH: &str = "kind_mismatch";
pub const CAT_MISSING_METADATA: &str = "missing_metadata";

/// Status constants.
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_ACCEPTED: &str = "accepted";
pub const STATUS_DISMISSED: &str = "dismissed";
