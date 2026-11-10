//! `KbCollectionDoc`: the KB-level `body_format` marker — how this KB's node
//! bodies are stored.
//!
//! Absent means MAE's own format, which is what every KB written before the key
//! existed holds. [`BODY_FORMAT_ORG_LITERAL_V1`] marks a KB seeded with literal
//! org bodies: typed fields (title, tags, properties in source order, the raw
//! `kind` string) lifted out of the header and the rest of each file kept
//! byte-for-byte. Clients that edit through `mae-org` preserve that format; MAE's
//! own node writer does not (it rewrites `[[id:X][d]]` links, lowercases
//! property keys, drops empty properties and folds any `kind` outside its enum to
//! `note`), so MAE refuses to write a KB carrying any `body_format` it does not
//! write itself. The refusal lives in mae-core (`kb_ops::literal_guard`); this
//! file only defines and reads the marker.
//!
//! A plain scalar at the top-level `collection` map, like `transport_policy` and
//! `encryption`: a peer built before this key existed ignores it (yrs maps are
//! open), and two concurrent first-sets converge last-writer-wins with no nested
//! container to race on.

use yrs::{Map, Transact};

use super::*;

/// `body_format` value of a KB whose node bodies are literal org text (v1).
pub const BODY_FORMAT_ORG_LITERAL_V1: &str = "org-literal-v1";

impl KbCollectionDoc {
    /// This KB's body format, or `None` for MAE's own format (the key absent, or
    /// set to the empty string).
    pub fn body_format(&self) -> Option<String> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_BODY_FORMAT_KEY)
            .map(|v| v.to_string(&txn))
            .filter(|s| !s.is_empty())
    }

    /// Set this KB's body format. Returns the encoded update. The empty string
    /// reads back as `None` (MAE's own format).
    pub fn set_body_format(&mut self, format: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_BODY_FORMAT_KEY, format);
        txn.encode_update_v1()
    }
}
