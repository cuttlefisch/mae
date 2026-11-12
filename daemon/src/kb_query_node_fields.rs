//! The typed fields `kb/query.get` and `kb/query.list` report for an
//! unencrypted node, built in one place so the two cannot disagree.
//!
//! Beyond ADR-093's schema-v2 fields:
//! - `org_header` (`{property_order, has_title_line, has_filetags_line}` or null) and
//!   `property_order` (its array, or null): the shape of the org header a
//!   seeded node came from, so a reader can lay out the drawer in file order.
//!   `properties` keeps its shape (an object, which has no order).
//! - `aliases`: what a reader should show, by the one rule in
//!   `mae_sync::kb::aliases` — in an `org-literal-v1` KB derived from the
//!   `ROAM_ALIASES` property, elsewhere the node's array — plus
//!   `aliases_problems` (strings) when that value could not be read cleanly.
//! - `created_at` and `modified_at`: RFC 3339 UTC, or null when unknown, with
//!   `modified_at_basis` saying where `modified_at` came from and, when the edit
//!   record could not be used, `modified_at_note` saying why.
//!
//! `modified_at` is the later of the seed's git modification time (the seed
//! record) and the latest edit recorded after the seed's own write (the editor
//! record, ADR-113 D3), whose times are the daemon's clock: display only, like
//! every time on that record.

use serde_json::{json, Map, Value};

use crate::doc_editors::EditorsSince;
use crate::doc_store::DocStore;
use crate::rfc3339;
use mae_sync::kb::KbNodeDoc;

/// Every field a reader shows for `doc`, without the body. `body_format` is
/// the KB collection's (`KbCollectionDoc::body_format`).
pub(super) async fn node_fields(
    store: &DocStore,
    kb_id: &str,
    node_id: &str,
    doc: &KbNodeDoc,
    body_format: Option<&str>,
) -> Map<String, Value> {
    let aliases = doc.aliases_in(body_format);
    let properties: Map<String, Value> = doc
        .properties()
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    let header = doc.org_header();
    let mut m = json!({
        "version": doc.version(),
        "schema_v": doc.schema_version(),
        "title": doc.title(),
        "kind": doc.kind(),
        "todo_state": doc.todo_state(),
        "priority": doc.priority(),
        "aliases": aliases.aliases,
        "properties": properties,
        "property_order": header.as_ref().map(|h| h.property_order.clone()),
        "org_header": header.as_ref().map(|h| json!({
            "property_order": h.property_order,
            "has_title_line": h.has_title_line,
            "has_filetags_line": h.has_filetags_line,
        })),
        "tags": doc.tags(),
        "created_at": doc.created_at().map(rfc3339::utc),
    });
    let fields = m.as_object_mut().expect("an object literal");
    if !aliases.problems.is_empty() {
        let problems: Vec<String> = aliases.problems.iter().map(|p| p.to_string()).collect();
        fields.insert("aliases_problems".into(), json!(problems));
    }
    fields.extend(modified(store, kb_id, node_id).await);
    std::mem::take(fields)
}

/// `modified_at`, `modified_at_basis` and (when the edit record is not usable)
/// `modified_at_note`.
async fn modified(store: &DocStore, kb_id: &str, node_id: &str) -> Map<String, Value> {
    let (seed, seed_note) = match store.seed_node(kb_id, node_id).await {
        Ok(row) => (row, None),
        Err(e) => (None, Some(format!("seed record unreadable: {e}"))),
    };
    let doc_name = mae_sync::kb_node_doc_name(kb_id, node_id);
    let cutoff = seed.as_ref().and_then(|r| r.seed_wal_id).unwrap_or(0);
    let (edited, edit_note) = latest_edit(store, &doc_name, cutoff).await;
    let seed_at = seed.as_ref().map(|r| r.modified_at);
    let (at, basis) = match (seed_at, edited) {
        (Some(s), Some(e)) if e > s => (Some(e), "edit"),
        (Some(s), _) => (Some(s), "seed"),
        (None, Some(e)) => (Some(e), "edit"),
        (None, None) => (None, "unknown"),
    };
    let mut m = Map::new();
    m.insert("modified_at".into(), json!(at.map(rfc3339::utc)));
    m.insert("modified_at_basis".into(), json!(basis));
    if let Some(note) = seed_note.or(edit_note) {
        m.insert("modified_at_note".into(), json!(note));
    }
    m
}

/// The latest edit time (unix seconds) after `cutoff`, or why there is none to
/// use. No edit since the cutoff is `(None, None)`: nothing to report, nothing
/// wrong.
async fn latest_edit(
    store: &DocStore,
    doc_name: &str,
    cutoff: u64,
) -> (Option<i64>, Option<String>) {
    let since = match store.editors_since_wal(doc_name, cutoff).await {
        Ok(EditorsSince::Incomplete {
            recorded_after_wal_id,
        }) if cutoff == 0 => {
            // A node from before the editor record: only edits after its
            // origin are dated, which is all there is to say.
            store
                .editors_since_wal(doc_name, recorded_after_wal_id)
                .await
        }
        other => other,
    };
    match since {
        Ok(EditorsSince::Known(rows)) => (
            rows.iter().map(|r| (r.last_at_ms / 1000) as i64).max(),
            None,
        ),
        Ok(EditorsSince::Incomplete { .. }) => (
            None,
            Some("the edit record does not cover this node's history".into()),
        ),
        Ok(EditorsSince::CutoffAhead { .. }) => (
            None,
            Some("the seed record's write is not in this node's edit record".into()),
        ),
        Err(e) => (None, Some(format!("edit record unreadable: {e}"))),
    }
}
