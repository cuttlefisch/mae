//! The `org_header` field of a node: the shape of the org file header a node
//! was seeded from (`org-literal-v1`), which the typed fields alone lose.
//!
//! The properties are a per-key `YMap` (so concurrent edits to different keys
//! merge), which has no order; the title is a plain `YText`, so an absent
//! `#+title:` line and an empty one read the same; and the tags are a list, so
//! an absent `#+filetags:` line and an empty one do too. An exporter writing
//! the file back needs all three, so they are recorded here:
//!
//! ```text
//! node.org_header: YMap {
//!     property_order: YArray<String>   the drawer's keys, in file order, as written
//!     has_title_line:     bool             the file had a `#+title:` line
//!     has_filetags_line:  bool             the file had a `#+filetags:` line
//! }
//! ```
//!
//! Optional, ADR-093 style: a document without it (v1, or a v2 node not made
//! by a seed) reads as `None`, "no header shape recorded", and nothing writes
//! it on read.
//!
//! @ai-caution: [crdt] Only the seeder writes this, on a document nobody else
//! holds yet, so the nested map is created in the same transaction as the
//! node. A future editor that maintains it on a SHARED document must not create
//! the map lazily (two peers would each insert their own map at the key and one
//! would be dropped, see `KbNodeDoc::new`), and must not rewrite the order
//! array wholesale on every save (concurrent saves would duplicate keys). Until
//! one does, a key added after the seed is simply missing from the order, and
//! the exporter places it after the ordered ones and reports it.

use yrs::{Any, Array, ArrayPrelim, Map, MapPrelim, Out, Transact};

use super::KbNodeDoc;

const ORG_HEADER_KEY: &str = "org_header";
const PROPERTY_ORDER_KEY: &str = "property_order";
const TITLE_LINE_KEY: &str = "has_title_line";
const FILETAGS_LINE_KEY: &str = "has_filetags_line";

/// The header shape of the org file a node was seeded from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrgHeader {
    /// The file drawer's keys in file order, each as written.
    pub property_order: Vec<String>,
    /// The file had a `#+title:` line (possibly empty).
    pub has_title_line: bool,
    /// The file had a `#+filetags:` line (possibly empty).
    pub has_filetags_line: bool,
}

impl KbNodeDoc {
    /// The recorded header shape, or `None` when the document has none.
    pub fn org_header(&self) -> Option<OrgHeader> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        let Some(Out::YMap(m)) = root.get(&txn, ORG_HEADER_KEY) else {
            return None;
        };
        let property_order = match m.get(&txn, PROPERTY_ORDER_KEY) {
            Some(Out::YArray(a)) => a.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        };
        let flag = |key: &str| matches!(m.get(&txn, key), Some(Out::Any(Any::Bool(true))));
        Some(OrgHeader {
            property_order,
            has_title_line: flag(TITLE_LINE_KEY),
            has_filetags_line: flag(FILETAGS_LINE_KEY),
        })
    }

    /// Record the header shape. Writes nothing when it is already recorded as
    /// `header` (no churn). See the module's `@ai-caution` before calling this
    /// on a document other peers hold.
    #[must_use = "dropping this update silently prevents the header shape from syncing to peers"]
    pub fn set_org_header(&mut self, header: &OrgHeader) -> Vec<u8> {
        if self.org_header().as_ref() == Some(header) {
            return self.doc.transact_mut().encode_update_v1();
        }
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        let m = root.insert(&mut txn, ORG_HEADER_KEY, MapPrelim::default());
        let order = m.insert(&mut txn, PROPERTY_ORDER_KEY, ArrayPrelim::default());
        for key in &header.property_order {
            order.push_back(&mut txn, key.as_str());
        }
        m.insert(&mut txn, TITLE_LINE_KEY, header.has_title_line);
        m.insert(&mut txn, FILETAGS_LINE_KEY, header.has_filetags_line);
        Self::stamp_schema_version(&root, &mut txn);
        txn.encode_update_v1()
    }
}

/// The header shape as bytes for a digest: `None` contributes nothing, so a
/// document without one hashes exactly as it did before the field existed.
pub(super) fn digest_bytes(header: Option<&OrgHeader>) -> Option<Vec<u8>> {
    let h = header?;
    let mut out = b"|org_header|".to_vec();
    out.push(u8::from(h.has_title_line));
    out.push(u8::from(h.has_filetags_line));
    out.extend((h.property_order.len() as u64).to_le_bytes());
    for key in &h.property_order {
        out.extend((key.len() as u64).to_le_bytes());
        out.extend(key.as_bytes());
    }
    Some(out)
}
