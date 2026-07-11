//! Free helper functions shared across the `cozo_store` query-domain modules:
//! `DataValue`/param conversions, row parsing, and UUID/error helpers.

use super::*;

pub(super) fn dv_str(s: &str) -> DataValue {
    DataValue::Str(s.into())
}

pub(super) fn kind_to_str(kind: NodeKind) -> &'static str {
    kind.as_str()
}

fn str_to_kind(s: &str) -> NodeKind {
    NodeKind::from_str_lossy(s)
}

/// Parse a CozoDB row [src, dst, rel_type, display, weight, confidence] into a Link.
pub(super) fn parse_link_row(row: &[DataValue]) -> Option<Link> {
    let src = row.first()?.get_str()?.to_string();
    let dst = row.get(1)?.get_str()?.to_string();
    let rel_type = row.get(2)?.get_str()?.to_string();
    let display_str = row.get(3)?.get_str().unwrap_or("");
    let display = if display_str.is_empty() {
        None
    } else {
        Some(display_str.to_string())
    };
    let weight = row.get(4).and_then(|v| v.get_float()).unwrap_or(1.0);
    let confidence = row.get(5).and_then(|v| v.get_float()).unwrap_or(1.0);
    Some(Link {
        src,
        dst,
        rel_type,
        display,
        weight,
        confidence,
    })
}

fn str_to_source(s: &str) -> Option<crate::NodeSource> {
    match s {
        "seed" => Some(crate::NodeSource::Seed),
        "user_org" => Some(crate::NodeSource::UserOrg),
        "manual" => Some(crate::NodeSource::Manual),
        "federation" => Some(crate::NodeSource::Federation),
        "" => None,
        _ => None,
    }
}

/// Generate a UUID v4 using std RandomState for entropy (no external crate needed).
pub(super) fn generate_uuid_v4() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    // Use two RandomState hashers seeded with different values for 128 bits of entropy
    let h1 = std::collections::hash_map::RandomState::new();
    let h2 = std::collections::hash_map::RandomState::new();
    let mut hasher1 = h1.build_hasher();
    hasher1.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
    );
    let mut hasher2 = h2.build_hasher();
    hasher2.write_u64(hasher1.finish().wrapping_add(0xdeadbeef));
    let val1 = hasher1.finish().to_le_bytes();
    let val2 = hasher2.finish().to_le_bytes();
    bytes[..8].copy_from_slice(&val1);
    bytes[8..].copy_from_slice(&val2);
    // Set version (4) and variant (10xx) bits
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

pub(super) fn cozo_err(e: impl std::fmt::Display) -> KbStoreError {
    KbStoreError::Storage(format!("CozoDB: {e}"))
}

pub(super) fn btree_params<const N: usize>(
    pairs: [(&str, DataValue); N],
) -> BTreeMap<String, DataValue> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// Convert a CozoDB row to a Node.
pub(super) fn row_to_node(row: &[DataValue]) -> Result<Node, KbStoreError> {
    let id = row
        .first()
        .and_then(|v| v.get_str())
        .ok_or_else(|| KbStoreError::Storage("missing id".into()))?
        .to_string();
    let title = row
        .get(1)
        .and_then(|v| v.get_str())
        .unwrap_or("")
        .to_string();
    let kind = row.get(2).and_then(|v| v.get_str()).unwrap_or("note");
    let body = row
        .get(3)
        .and_then(|v| v.get_str())
        .unwrap_or("")
        .to_string();
    let tags_json = row.get(4).and_then(|v| v.get_str()).unwrap_or("[]");
    let todo_state = row.get(5).and_then(|v| v.get_str()).and_then(|s| {
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    });
    let priority = row
        .get(6)
        .and_then(|v| v.get_str())
        .and_then(|s| s.chars().next());
    let source = row.get(7).and_then(|v| v.get_str()).and_then(str_to_source);
    let source_version =
        row.get(8)
            .and_then(|v| v.get_int())
            .and_then(|i| if i == 0 { None } else { Some(i as u32) });
    let aliases_json = row.get(9).and_then(|v| v.get_str()).unwrap_or("[]");
    let properties_json = row.get(10).and_then(|v| v.get_str()).unwrap_or("{}");
    let has_crdt = row.get(12).and_then(|v| v.get_bool()).unwrap_or(false);
    let crdt_doc = if has_crdt {
        row.get(11).and_then(|v| v.get_bytes().map(|b| b.to_vec()))
    } else {
        None
    };

    let tags: Vec<String> = serde_json::from_str(tags_json).unwrap_or_default();
    let aliases: Vec<String> = serde_json::from_str(aliases_json).unwrap_or_default();
    let properties: std::collections::HashMap<String, String> =
        serde_json::from_str(properties_json).unwrap_or_default();

    let mut node = Node::new(id, title, str_to_kind(kind), body)
        .with_tags(tags)
        .with_aliases(aliases)
        .with_properties(properties);
    node.todo_state = todo_state;
    node.priority = priority;
    node.source = source;
    node.source_version = source_version;
    node.crdt_doc = crdt_doc;
    Ok(node)
}
