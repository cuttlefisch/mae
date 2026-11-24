//! The node document's field types, checked on write (the schema readers are
//! tolerant of, but writers must keep). Every key the node map defines, with
//! the type MAE writes it as; a key not listed is left alone, so a newer
//! client's additive field never trips an older daemon.
//!
//! | key | type |
//! |---|---|
//! | `title`, `body` | `Y.Text` |
//! | `tags`, `links`, `aliases` | `Y.Array` of strings |
//! | `props` | `Y.Map` of string values |
//! | `id`, `kind`, `todo`, `prio`, `source`, `src_v`, `schema_v`, `created` | string |
//! | `meta` | `Y.Map` (legacy; its values are not checked) |
//! | `org_header` | `Y.Map`: `property_order` an array of strings, `has_title_line` and `has_filetags_line` booleans |
//!
//! And the `attachments` root (ADR-115 D4), reported as `attachments`: every
//! entry a plain-file NAME bound to `{sha256, size, media_type}` of the right
//! shapes. Extra fields in an entry are not a violation. Unlike the other
//! fields it is checked entry by entry: a map that already holds one bad entry
//! does not let a writer add another.

use std::collections::BTreeSet;

use yrs::{Any, Array, Map, Out, Transact};

use super::KbNodeDoc;
use crate::SyncError;

const TEXTS: &[&str] = &["title", "body"];
const STRING_ARRAYS: &[&str] = &["tags", "links", "aliases"];
/// How a bad `attachments` entry is reported.
const ATTACHMENTS: &str = "attachments";
const SCALARS: &[&str] = &[
    "id", "kind", "todo", "prio", "source", "src_v", "schema_v", "created",
];

fn is_string(v: &Out) -> bool {
    matches!(v, Out::Any(Any::String(_)))
}

impl KbNodeDoc {
    /// The known fields whose value has the wrong type. Absent is never wrong.
    pub fn field_type_violations(&self) -> BTreeSet<&'static str> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        let mut bad = BTreeSet::new();
        for &k in TEXTS {
            if root
                .get(&txn, k)
                .is_some_and(|v| !matches!(v, Out::YText(_)))
            {
                bad.insert(k);
            }
        }
        for &k in STRING_ARRAYS {
            let ok = match root.get(&txn, k) {
                None => true,
                Some(Out::YArray(a)) => a.iter(&txn).all(|v| is_string(&v)),
                Some(_) => false,
            };
            if !ok {
                bad.insert(k);
            }
        }
        for &k in SCALARS {
            if root.get(&txn, k).is_some_and(|v| !is_string(&v)) {
                bad.insert(k);
            }
        }
        let props_ok = match root.get(&txn, "props") {
            None => true,
            Some(Out::YMap(m)) => m.iter(&txn).all(|(_, v)| is_string(&v)),
            Some(_) => false,
        };
        if !props_ok {
            bad.insert("props");
        }
        if root
            .get(&txn, "meta")
            .is_some_and(|v| !matches!(v, Out::YMap(_)))
        {
            bad.insert("meta");
        }
        let header_ok = match root.get(&txn, "org_header") {
            None => true,
            Some(Out::YMap(m)) => {
                let order = match m.get(&txn, "property_order") {
                    None => true,
                    Some(Out::YArray(a)) => a.iter(&txn).all(|v| is_string(&v)),
                    Some(_) => false,
                };
                let flag = |k| {
                    m.get(&txn, k)
                        .is_none_or(|v| matches!(v, Out::Any(Any::Bool(_))))
                };
                order && flag("has_title_line") && flag("has_filetags_line")
            }
            Some(_) => false,
        };
        if !header_ok {
            bad.insert("org_header");
        }
        drop(txn);
        if !self.rejected_attachment_entries().is_empty() {
            bad.insert(ATTACHMENTS);
        }
        bad
    }

    /// The first known field `update` would leave mistyped on the document
    /// whose state is `base` (empty: a new document), that was NOT already
    /// mistyped there. `None` when the update keeps every field's type, or
    /// only touches fields that were already wrong, so a legacy document
    /// stays writable. `Err` when either does not decode.
    ///
    /// `attachments` is judged per entry: the update introduces a bad entry
    /// when an entry is rejected after it that was not rejected before with
    /// the same value, even if another NAME already was.
    pub fn field_type_introduced(
        base: &[u8],
        update: &[u8],
    ) -> Result<Option<&'static str>, SyncError> {
        let mut doc = if base.is_empty() {
            KbNodeDoc {
                doc: crate::text::new_doc(),
            }
        } else {
            KbNodeDoc::from_bytes(base)?
        };
        let before = doc.field_type_violations();
        let rejected_before = doc.rejected_attachment_entries();
        doc.apply_update(update)?;
        Ok(introduced(&doc, &before, &rejected_before))
    }

    /// As [`field_type_introduced`](Self::field_type_introduced), for an
    /// update that REPLACES the document whose state is `existing` (a rebirth
    /// or a reseal): the result is `replacement` alone, and what was already
    /// mistyped is judged against the document it replaces, not against
    /// nothing. A rebirth copies the attachments root raw, so a node holding
    /// a legacy entry is reborn with it; judged against an empty base, that
    /// entry would read as introduced and the node could never be reborn.
    /// An entry the replaced document did not hold still counts, and an
    /// `existing` that does not decode excuses nothing (it is judged as
    /// empty). `Err` when `replacement` does not decode.
    pub fn field_type_introduced_replacing(
        existing: &[u8],
        replacement: &[u8],
    ) -> Result<Option<&'static str>, SyncError> {
        let (before, rejected_before) = match KbNodeDoc::from_bytes(existing) {
            Ok(old) => (
                old.field_type_violations(),
                old.rejected_attachment_entries(),
            ),
            Err(_) => Default::default(),
        };
        let mut doc = KbNodeDoc {
            doc: crate::text::new_doc(),
        };
        doc.apply_update(replacement)?;
        Ok(introduced(&doc, &before, &rejected_before))
    }
}

/// The first violation of `doc` not in `before`, counting `attachments` as
/// introduced when it rejects an entry (NAME and value) `rejected_before` did
/// not hold.
fn introduced(
    doc: &KbNodeDoc,
    before: &BTreeSet<&'static str>,
    rejected_before: &BTreeSet<(String, Option<String>)>,
) -> Option<&'static str> {
    let new_bad_entry = !doc.rejected_attachment_entries().is_subset(rejected_before);
    doc.field_type_violations()
        .into_iter()
        .find(|k| !before.contains(k) || (*k == ATTACHMENTS && new_bad_entry))
}
