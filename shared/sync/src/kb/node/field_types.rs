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

use std::collections::BTreeSet;

use yrs::{Any, Array, Map, Out, Transact};

use super::KbNodeDoc;
use crate::SyncError;

const TEXTS: &[&str] = &["title", "body"];
const STRING_ARRAYS: &[&str] = &["tags", "links", "aliases"];
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
        bad
    }

    /// The first known field `update` would leave mistyped on the document
    /// whose state is `base` (empty: a new document), that was NOT already
    /// mistyped there. `None` when the update keeps every field's type, or
    /// only touches fields that were already wrong, so a legacy document
    /// stays writable. `Err` when either does not decode.
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
        doc.apply_update(update)?;
        Ok(doc
            .field_type_violations()
            .into_iter()
            .find(|k| !before.contains(k)))
    }
}
