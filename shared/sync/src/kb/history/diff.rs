//! The "what" of a change (D3): the difference between two versions, computed
//! when asked for, never stored.

use serde_json::{json, Value};

use super::MaterializedNode;

/// One field that differs between two versions. Text fields carry a unified
/// line diff; every other field carries its value before and after.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldChange {
    pub field: &'static str,
    pub change: Value,
}

fn unified(a: &str, b: &str) -> Value {
    let diff = similar::TextDiff::from_lines(a, b);
    json!({ "unified": diff.unified_diff().context_radius(3).to_string() })
}

fn values(a: Value, b: Value) -> Option<Value> {
    (a != b).then(|| json!({ "from": a, "to": b }))
}

/// Every field that differs from `a` to `b`, in `MaterializedNode` order.
/// Properties compare as a map: their stored order means nothing.
pub fn diff(a: &MaterializedNode, b: &MaterializedNode) -> Vec<FieldChange> {
    // No `..`: a field added to MaterializedNode must be compared too.
    let MaterializedNode {
        id,
        title,
        body,
        tags,
        links,
        kind,
        todo_state,
        priority,
        aliases,
        properties,
        source_version,
        source,
        created_at,
        org_header,
    } = a;
    let header = |h: &Option<super::super::OrgHeader>| {
        h.as_ref().map(|h| {
            json!({
                "property_order": h.property_order,
                "has_title_line": h.has_title_line,
                "has_filetags_line": h.has_filetags_line,
            })
        })
    };
    let candidates: Vec<(&'static str, Option<Value>)> = vec![
        ("id", values(json!(id), json!(b.id))),
        (
            "title",
            (title != &b.title).then(|| unified(title, &b.title)),
        ),
        ("body", (body != &b.body).then(|| unified(body, &b.body))),
        ("tags", values(json!(tags), json!(b.tags))),
        ("links", values(json!(links), json!(b.links))),
        ("kind", values(json!(kind), json!(b.kind))),
        ("todo_state", values(json!(todo_state), json!(b.todo_state))),
        ("priority", values(json!(priority), json!(b.priority))),
        ("aliases", values(json!(aliases), json!(b.aliases))),
        ("properties", values(json!(properties), json!(b.properties))),
        (
            "source_version",
            values(json!(source_version), json!(b.source_version)),
        ),
        ("source", values(json!(source), json!(b.source))),
        ("created_at", values(json!(created_at), json!(b.created_at))),
        (
            "org_header",
            values(json!(header(org_header)), json!(header(&b.org_header))),
        ),
    ];
    candidates
        .into_iter()
        .filter_map(|(field, change)| change.map(|change| FieldChange { field, change }))
        .collect()
}
