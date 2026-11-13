//! The typed fields `kb/query.get` and `kb/query.list` report for an
//! unencrypted node, built in one place so the two cannot disagree.
//!
//! Beyond ADR-093's schema-v2 fields:
//! - `org_header` (`{property_order, has_title_line, has_filetags_line}` or null) and
//!   `property_order` (its array, or null): the shape of the org header a
//!   seeded node came from, so a reader can lay out the drawer in file order.
//!   `properties` keeps its shape (an object, which has no order).
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
use mae_sync::kb::{KbNodeDoc, MaterializedNode};

/// Every field a reader shows for `doc`, without the body.
pub(super) async fn node_fields(
    store: &DocStore,
    kb_id: &str,
    node_id: &str,
    doc: &KbNodeDoc,
) -> Map<String, Value> {
    let mut m = content_fields(&doc.materialize());
    m.insert("version".into(), json!(doc.version()));
    m.insert("schema_v".into(), json!(doc.schema_version()));
    m.extend(modified(store, kb_id, node_id).await);
    m
}

/// The fields a reader shows for a node's content, without the body: shared
/// by the live node (`kb/query.get`/`list`) and a version of it
/// (`kb/history.get`, ADR-116), so the two cannot render a node differently.
pub(crate) fn content_fields(n: &MaterializedNode) -> Map<String, Value> {
    let properties: Map<String, Value> = n
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let header = n.org_header.as_ref();
    let mut m = json!({
        "title": n.title,
        "kind": n.kind,
        "todo_state": n.todo_state,
        "priority": n.priority,
        "aliases": n.aliases,
        "properties": properties,
        "property_order": header.map(|h| h.property_order.clone()),
        "org_header": header.map(|h| json!({
            "property_order": h.property_order,
            "has_title_line": h.has_title_line,
            "has_filetags_line": h.has_filetags_line,
        })),
        "tags": n.tags,
        "created_at": n.created_at.map(rfc3339::utc),
    });
    std::mem::take(m.as_object_mut().expect("an object literal"))
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
