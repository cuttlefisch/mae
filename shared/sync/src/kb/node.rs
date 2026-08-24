//! `KbNodeDoc`: yrs-backed KB node with YMap schema.
//!
//! All yrs Doc instances use UTF-16 offset kind (via `text::new_doc()`) for
//! consistency with the Yjs standard. See the CRDT UTF-16 fix (92a20b8).

use sha2::{Digest, Sha256};
use yrs::{
    updates::decoder::Decode, updates::encoder::Encode, Array, ArrayPrelim, Doc, GetString, Map,
    MapPrelim, Out, ReadTxn, TextPrelim, Transact,
};

use crate::text::{new_doc, new_doc_with_client_id};
use crate::SyncError;

use super::sv_has_ops_beyond;

const ID_KEY: &str = "id";
const TITLE_KEY: &str = "title";
const BODY_KEY: &str = "body";
const TAGS_KEY: &str = "tags";
const LINKS_KEY: &str = "links";
const META_KEY: &str = "meta";

// --- schema v2 (ADR-093) ---
//
// @ai-caution: [crdt] These keys are OPTIONAL by design and readers must tolerate
// their absence — a v1 document simply has none of them. Do NOT add an
// "upcast on read" that writes them when a v1 doc is opened. In a CRDT that is a
// live hazard, not a convenience: two peers opening the same v1 document would each
// author their own migration ops, and Automerge's own docs name this as the thing
// that makes CRDT schema migration harder than a centralized one ("two users
// independently perform the same migration… you need to ensure the two migrations
// don't clash"). Writing these fields only when the application writes the node
// anyway means there is no migration op to clash. The one-time bulk backfill is a
// deliberate single-writer pass (ADR-094), not a read-triggered side effect.
const SCHEMA_VERSION_KEY: &str = "schema_v";
const KIND_KEY: &str = "kind";
const TODO_KEY: &str = "todo";
const PRIORITY_KEY: &str = "prio";
const ALIASES_KEY: &str = "aliases";
const PROPS_KEY: &str = "props";
const SOURCE_VERSION_KEY: &str = "src_v";

/// Current node schema version. Absent ⇒ v1 (text fields only).
pub const NODE_SCHEMA_VERSION: i64 = 2;

/// Materialized content from a KbNodeDoc — all fields extracted for FTS rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedNode {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    /// v2 fields — defaults when reading a v1 document.
    pub kind: Option<String>,
    pub todo_state: Option<String>,
    pub priority: Option<String>,
    pub aliases: Vec<String>,
    pub properties: std::collections::HashMap<String, String>,
    pub source_version: Option<u32>,
}

/// A KB node represented as a yrs document.
///
/// Schema:
/// - Root YMap "node" contains: id (String), title (YText), body (YText),
///   tags (YArray<String>), links (YArray<String>), meta (YMap<String, String>)
///
/// All Doc instances use UTF-16 offset kind for cross-client consistency.
pub struct KbNodeDoc {
    doc: Doc,
}

impl KbNodeDoc {
    /// Create a new KB node document with UTF-16 offset kind.
    pub fn new(id: &str, title: &str, body: &str, tags: &[String]) -> Self {
        let doc = new_doc();
        {
            let root = doc.get_or_insert_map("node");
            let mut txn = doc.transact_mut();

            root.insert(&mut txn, ID_KEY, id);
            root.insert(&mut txn, TITLE_KEY, TextPrelim::new(title));
            root.insert(&mut txn, BODY_KEY, TextPrelim::new(body));

            let tags_arr = root.insert(&mut txn, TAGS_KEY, ArrayPrelim::default());
            for tag in tags {
                tags_arr.push_back(&mut txn, tag.as_str());
            }

            root.insert(&mut txn, LINKS_KEY, ArrayPrelim::default());
            root.insert(&mut txn, META_KEY, MapPrelim::default());
            // ADR-093 + ADR-033: seed the v2 containers EAGERLY. Creating a nested
            // container lazily on first write is unsafe under concurrency — two
            // peers each insert their own fresh map/array at the same key, one
            // wins, and the loser's entries are silently dropped. Same reasoning as
            // COLL_LEASE_KEY in collection_core.rs.
            root.insert(&mut txn, ALIASES_KEY, ArrayPrelim::default());
            root.insert(&mut txn, PROPS_KEY, MapPrelim::default());
        }
        Self { doc }
    }

    /// Create a new KB node document with a specific client ID for collaborative use.
    pub fn new_with_client_id(
        id: &str,
        title: &str,
        body: &str,
        tags: &[String],
        client_id: u64,
    ) -> Self {
        let doc = new_doc_with_client_id(client_id);
        {
            let root = doc.get_or_insert_map("node");
            let mut txn = doc.transact_mut();

            root.insert(&mut txn, ID_KEY, id);
            root.insert(&mut txn, TITLE_KEY, TextPrelim::new(title));
            root.insert(&mut txn, BODY_KEY, TextPrelim::new(body));

            let tags_arr = root.insert(&mut txn, TAGS_KEY, ArrayPrelim::default());
            for tag in tags {
                tags_arr.push_back(&mut txn, tag.as_str());
            }

            root.insert(&mut txn, LINKS_KEY, ArrayPrelim::default());
            root.insert(&mut txn, META_KEY, MapPrelim::default());
            // ADR-093 + ADR-033: seed the v2 containers EAGERLY. Creating a nested
            // container lazily on first write is unsafe under concurrency — two
            // peers each insert their own fresh map/array at the same key, one
            // wins, and the loser's entries are silently dropped. Same reasoning as
            // COLL_LEASE_KEY in collection_core.rs.
            root.insert(&mut txn, ALIASES_KEY, ArrayPrelim::default());
            root.insert(&mut txn, PROPS_KEY, MapPrelim::default());
        }
        Self { doc }
    }

    /// Load from encoded bytes with UTF-16 offset kind.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SyncError> {
        let doc = new_doc();
        let update =
            yrs::Update::decode_v1(bytes).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        Ok(Self { doc })
    }

    /// Load from encoded bytes with a specific client ID for joining a collaborative KB.
    pub fn from_bytes_with_client_id(bytes: &[u8], client_id: u64) -> Result<Self, SyncError> {
        let doc = new_doc_with_client_id(client_id);
        let update =
            yrs::Update::decode_v1(bytes).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        Ok(Self { doc })
    }

    /// Encode full state for persistence.
    pub fn encode(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
    }

    /// Alias for `encode()` — naming consistency with TextSync.
    pub fn encode_state(&self) -> Vec<u8> {
        self.encode()
    }

    /// Compute an incremental diff against a remote state vector.
    ///
    /// Returns only the updates the remote doesn't have yet. More efficient
    /// than sending the full state when the remote is only slightly behind.
    pub fn encode_diff(&self, remote_sv: &[u8]) -> Result<Vec<u8>, SyncError> {
        let sv = yrs::StateVector::decode_v1(remote_sv)
            .map_err(|e| SyncError::Encoding(e.to_string()))?;
        let txn = self.doc.transact();
        Ok(txn.encode_state_as_update_v1(&sv))
    }

    /// Get the node ID.
    pub fn id(&self) -> String {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        root.get(&txn, ID_KEY)
            .map(|v| v.to_string(&txn))
            .unwrap_or_default()
    }

    /// Get title.
    pub fn title(&self) -> String {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, TITLE_KEY) {
            Some(Out::YText(text)) => text.get_string(&txn),
            _ => String::new(),
        }
    }

    /// Set title. Returns encoded update.
    #[must_use = "dropping this update silently prevents the title change from syncing to peers"]
    pub fn set_title(&mut self, title: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YText(text)) = root.get(&txn, TITLE_KEY) {
            let current = text.get_string(&txn);
            crate::text::reconcile_text_ref(&mut txn, &text, &current, title);
        }
        txn.encode_update_v1()
    }

    /// Get body.
    pub fn body(&self) -> String {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, BODY_KEY) {
            Some(Out::YText(text)) => text.get_string(&txn),
            _ => String::new(),
        }
    }

    /// Set body. Returns encoded update.
    #[must_use = "dropping this update silently prevents the body change from syncing to peers"]
    pub fn set_body(&mut self, body: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YText(text)) = root.get(&txn, BODY_KEY) {
            let current = text.get_string(&txn);
            crate::text::reconcile_text_ref(&mut txn, &text, &current, body);
        }
        txn.encode_update_v1()
    }

    /// Get tags.
    pub fn tags(&self) -> Vec<String> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, TAGS_KEY) {
            Some(Out::YArray(arr)) => arr.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        }
    }

    /// Add a tag. Returns encoded update.
    #[must_use = "dropping this update silently prevents the added tag from syncing to peers"]
    pub fn add_tag(&mut self, tag: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(arr)) = root.get(&txn, TAGS_KEY) {
            arr.push_back(&mut txn, tag);
        }
        txn.encode_update_v1()
    }

    /// Remove a tag by value. Returns encoded update.
    pub fn remove_tag(&mut self, tag: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(arr)) = root.get(&txn, TAGS_KEY) {
            let idx = arr.iter(&txn).position(|v| v.to_string(&txn) == tag);
            if let Some(idx) = idx {
                arr.remove(&mut txn, idx as u32);
            }
        }
        txn.encode_update_v1()
    }

    /// Set the tag list to `tags`, emitting only the removals and additions that
    /// actually differ. Returns the encoded update. This is the setter
    /// `upsert_with_crdt` needs for a wholesale tag edit (e.g. `kb_update` with a
    /// new tags list) to enter the CRDT and broadcast a delta — B-18: previously
    /// only `set_title`/`set_body` were wired, so tag changes after node creation
    /// never synced (peer apply was a no-op).
    ///
    /// @ai-caution: [crdt] Do NOT restore the old `remove_range(0, len)` +
    /// re-append. It is the `YArray` form of the `set_body` bug (ADR-092 D2): two
    /// peers each adding one tag both wipe the array and re-append their own full
    /// list, so every tag they had in common returns once per peer
    /// (`["rust","kb","from-a","rust","kb","from-b"]`). Tags are a set, so the
    /// diff is by value, not by position. Guarded by
    /// `concurrent_tag_edits_do_not_duplicate_shared_tags`.
    pub fn set_tags(&mut self, tags: &[String]) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(arr)) = root.get(&txn, TAGS_KEY) {
            let current: Vec<String> = arr.iter(&txn).map(|v| v.to_string(&txn)).collect();
            // Drop what is no longer wanted, highest index first so the remaining
            // indices stay valid. A duplicate already in the array is dropped too —
            // only its first occurrence is retained.
            let mut kept: Vec<&String> = Vec::new();
            for (i, existing) in current.iter().enumerate().rev() {
                if tags.contains(existing) && !kept.contains(&existing) {
                    kept.push(existing);
                } else {
                    arr.remove(&mut txn, i as u32);
                }
            }
            // Append the ones that are wanted but absent, in the caller's order.
            for tag in tags {
                if !kept.contains(&tag) {
                    arr.push_back(&mut txn, tag.as_str());
                    kept.push(tag);
                }
            }
        }
        txn.encode_update_v1()
    }

    // ------------------------------------------------------------------
    // Schema v2 (ADR-093) — every remaining `mae_kb::Node` field.
    //
    // Readers below all tolerate an absent key, which is what makes a v1
    // document readable under v2 without any migration write. See the
    // `@ai-caution` on the key constants for why that matters.
    // ------------------------------------------------------------------

    /// Stamp the schema version, but **only when it would actually change**.
    ///
    /// @ai-caution: [crdt-growth] `schema_v` is a CONSTANT. Re-inserting it on
    /// every field change made it the hottest key in the document, and every
    /// overwrite of a `Y.Map` key retires the previous Item permanently — yrs has
    /// no way to reclaim those (see the upstream tombstone-leak issue on exactly
    /// this `Y.Map` shape). MAE's own workload made that acute: activity tracking
    /// used to write on every node READ, so a document accrued two un-reclaimable
    /// tombstones per read, forever, on the one field class with no compaction
    /// story. Activity moved out of node content in #729; this removes the other
    /// half.
    ///
    /// Reading before writing is not an optimisation here — an unconditional
    /// write is unbounded growth for zero information.
    fn stamp_schema_version(root: &yrs::MapRef, txn: &mut yrs::TransactionMut) {
        use yrs::Map;
        let current = root
            .get(txn, SCHEMA_VERSION_KEY)
            .and_then(|v| v.to_string(txn).parse::<i64>().ok());
        if current != Some(NODE_SCHEMA_VERSION) {
            root.insert(txn, SCHEMA_VERSION_KEY, NODE_SCHEMA_VERSION.to_string());
        }
    }

    /// Schema version of this document. **1** when the key is absent — i.e. a
    /// document authored before ADR-093, carrying text fields only.
    pub fn schema_version(&self) -> i64 {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        root.get(&txn, SCHEMA_VERSION_KEY)
            .and_then(|v| v.to_string(&txn).parse::<i64>().ok())
            .unwrap_or(1)
    }

    /// Read an optional scalar string field. Absent ⇒ `None`.
    fn scalar(&self, key: &str) -> Option<String> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        root.get(&txn, key)
            .map(|v| v.to_string(&txn))
            .filter(|s| !s.is_empty())
    }

    /// Set (or clear) an optional scalar string field, stamping the schema
    /// version. Writes nothing when the value is already correct — an unchanged
    /// save must not churn tombstones into a replicated document.
    fn set_scalar(&mut self, key: &str, value: Option<&str>) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        let current = root.get(&txn, key).map(|v| v.to_string(&txn));
        let target = value.map(|s| s.to_string());
        if current.as_deref().filter(|s| !s.is_empty()) != target.as_deref() {
            match &target {
                Some(v) => {
                    root.insert(&mut txn, key, v.as_str());
                }
                None => {
                    root.remove(&mut txn, key);
                }
            }
            Self::stamp_schema_version(&root, &mut txn);
        }
        txn.encode_update_v1()
    }

    /// Node kind (`mae_kb::NodeKind`, as its serialized string). Absent ⇒ `None`.
    pub fn kind(&self) -> Option<String> {
        self.scalar(KIND_KEY)
    }

    /// Set the node kind. Returns the encoded update.
    #[must_use = "dropping this update silently prevents the kind change from syncing to peers"]
    pub fn set_kind(&mut self, kind: Option<&str>) -> Vec<u8> {
        self.set_scalar(KIND_KEY, kind)
    }

    /// Org todo state (`TODO`, `NEXT`, `DONE`, …). Absent ⇒ `None`.
    pub fn todo_state(&self) -> Option<String> {
        self.scalar(TODO_KEY)
    }

    /// Set the todo state. Returns the encoded update.
    #[must_use = "dropping this update silently prevents the todo change from syncing to peers"]
    pub fn set_todo_state(&mut self, todo: Option<&str>) -> Vec<u8> {
        self.set_scalar(TODO_KEY, todo)
    }

    /// Org priority cookie (`A`/`B`/`C`). Absent ⇒ `None`.
    pub fn priority(&self) -> Option<String> {
        self.scalar(PRIORITY_KEY)
    }

    /// Set the priority. Returns the encoded update.
    #[must_use = "dropping this update silently prevents the priority change from syncing to peers"]
    pub fn set_priority(&mut self, priority: Option<&str>) -> Vec<u8> {
        self.set_scalar(PRIORITY_KEY, priority)
    }

    /// Seed-content version stamp. Absent ⇒ `None`.
    pub fn source_version(&self) -> Option<u32> {
        self.scalar(SOURCE_VERSION_KEY)
            .and_then(|s| s.parse::<u32>().ok())
    }

    /// Set the source version. Returns the encoded update.
    #[must_use = "dropping this update silently prevents the version change from syncing to peers"]
    pub fn set_source_version(&mut self, v: Option<u32>) -> Vec<u8> {
        self.set_scalar(SOURCE_VERSION_KEY, v.map(|n| n.to_string()).as_deref())
    }

    /// Aliases. Absent ⇒ empty.
    pub fn aliases(&self) -> Vec<String> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, ALIASES_KEY) {
            Some(Out::YArray(arr)) => arr.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        }
    }

    /// Set the alias list, diffing **by value** exactly as `set_tags` does.
    ///
    /// @ai-caution: [crdt] Not a clear-and-refill. Two peers each adding one alias
    /// would otherwise both wipe the array and re-append their own full list, so
    /// every alias they had in common returns once per peer — the `YArray` form of
    /// the ADR-092 D2 bug.
    #[must_use = "dropping this update silently prevents the alias change from syncing to peers"]
    pub fn set_aliases(&mut self, aliases: &[String]) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        let arr = match root.get(&txn, ALIASES_KEY) {
            Some(Out::YArray(a)) => a,
            _ => root.insert(&mut txn, ALIASES_KEY, ArrayPrelim::default()),
        };
        let current: Vec<String> = arr.iter(&txn).map(|v| v.to_string(&txn)).collect();
        let mut kept: Vec<&String> = Vec::new();
        let mut changed = false;
        for (i, existing) in current.iter().enumerate().rev() {
            if aliases.contains(existing) && !kept.contains(&existing) {
                kept.push(existing);
            } else {
                arr.remove(&mut txn, i as u32);
                changed = true;
            }
        }
        for a in aliases {
            if !kept.contains(&a) {
                arr.push_back(&mut txn, a.as_str());
                kept.push(a);
                changed = true;
            }
        }
        // Stamp the version only on a real change — an unchanged save must not
        // author an op (ADR-092 D2's no-churn rule).
        if changed {
            Self::stamp_schema_version(&root, &mut txn);
        }
        txn.encode_update_v1()
    }

    /// Properties (the org `:PROPERTIES:` drawer — where org-roam's `:ID:` and
    /// `:ROLE:` live). Absent ⇒ empty.
    pub fn properties(&self) -> std::collections::HashMap<String, String> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, PROPS_KEY) {
            Some(Out::YMap(m)) => m
                .iter(&txn)
                .map(|(k, v)| (k.to_string(), v.to_string(&txn)))
                .collect(),
            _ => std::collections::HashMap::new(),
        }
    }

    /// Set properties, **per key**.
    ///
    /// @ai-caution: [crdt] A `YMap` is used, and updated key-by-key, precisely so
    /// that two peers editing DIFFERENT properties merge instead of clobbering.
    /// Never reduce this to "clear the map, re-insert everything" — that is the
    /// same defect as the old `set_tags`/`set_body`, and here it would silently
    /// discard a concurrent peer's unrelated property edit.
    #[must_use = "dropping this update silently prevents the property change from syncing to peers"]
    pub fn set_properties(&mut self, props: &std::collections::HashMap<String, String>) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        let map = match root.get(&txn, PROPS_KEY) {
            Some(Out::YMap(m)) => m,
            _ => root.insert(&mut txn, PROPS_KEY, MapPrelim::default()),
        };
        let current: Vec<(String, String)> = map
            .iter(&txn)
            .map(|(k, v)| (k.to_string(), v.to_string(&txn)))
            .collect();
        let mut changed = false;
        for (k, _) in current.iter().filter(|(k, _)| !props.contains_key(k)) {
            map.remove(&mut txn, k.as_str());
            changed = true;
        }
        for (k, v) in props {
            let unchanged = current.iter().any(|(ck, cv)| ck == k && cv == v);
            if !unchanged {
                map.insert(&mut txn, k.as_str(), v.as_str());
                changed = true;
            }
        }
        // Stamp the version only on a real change — an unchanged save must not
        // author an op (ADR-092 D2's no-churn rule).
        if changed {
            Self::stamp_schema_version(&root, &mut txn);
        }
        txn.encode_update_v1()
    }

    /// Get links.
    pub fn links(&self) -> Vec<String> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, LINKS_KEY) {
            Some(Out::YArray(arr)) => arr.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        }
    }

    /// Add a link. Returns encoded update.
    #[must_use = "dropping this update silently prevents the added link from syncing to peers"]
    pub fn add_link(&mut self, target: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(arr)) = root.get(&txn, LINKS_KEY) {
            arr.push_back(&mut txn, target);
        }
        txn.encode_update_v1()
    }

    /// Remove a link by target. Returns encoded update.
    pub fn remove_link(&mut self, target: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(arr)) = root.get(&txn, LINKS_KEY) {
            let idx = arr.iter(&txn).position(|v| v.to_string(&txn) == target);
            if let Some(idx) = idx {
                arr.remove(&mut txn, idx as u32);
            }
        }
        txn.encode_update_v1()
    }

    /// Set a metadata key-value pair. Returns encoded update.
    pub fn set_meta(&mut self, key: &str, value: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(meta)) = root.get(&txn, META_KEY) {
            meta.insert(&mut txn, key, value);
        }
        txn.encode_update_v1()
    }

    /// Get a metadata value by key.
    pub fn get_meta(&self, key: &str) -> Option<String> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, META_KEY) {
            Some(Out::YMap(meta)) => meta.get(&txn, key).map(|v| v.to_string(&txn)),
            _ => None,
        }
    }

    /// Apply a remote update. Returns whether content actually changed
    /// (detected via SHA-256 hash comparison, since yrs state vectors are
    /// monotonically increasing even for undo operations).
    pub fn apply_update(&mut self, update: &[u8]) -> Result<bool, SyncError> {
        let hash_before = self.content_hash();
        let update =
            yrs::Update::decode_v1(update).map_err(|e| SyncError::Encoding(e.to_string()))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|e| SyncError::Encoding(e.to_string()))?;
        drop(txn);
        let hash_after = self.content_hash();
        Ok(hash_before != hash_after)
    }

    /// State vector for sync.
    pub fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    /// True if this document holds operations not yet covered by `remote_sv`
    /// — i.e. `encode_diff(remote_sv)` would carry real (non-no-op) content.
    ///
    /// Format-independent: a yrs v1 update against a fully-covering state vector
    /// still encodes to a small non-empty byte sequence (`[0, 0]`), so checking
    /// `encode_diff(..).is_empty()` is wrong. We instead compare state vectors
    /// per client: we are "ahead" iff some client's local clock exceeds what the
    /// remote has seen. Used by ADR-022 reconcile to decide whether a local-ahead
    /// push is actually needed.
    pub fn has_ops_beyond(&self, remote_sv: &[u8]) -> Result<bool, SyncError> {
        sv_has_ops_beyond(&self.state_vector(), remote_sv)
    }

    /// Extract all fields into a `MaterializedNode` for FTS5 rebuild.
    pub fn materialize(&self) -> MaterializedNode {
        MaterializedNode {
            id: self.id(),
            title: self.title(),
            body: self.body(),
            tags: self.tags(),
            links: self.links(),
            kind: self.kind(),
            todo_state: self.todo_state(),
            priority: self.priority(),
            aliases: self.aliases(),
            properties: self.properties(),
            source_version: self.source_version(),
        }
    }

    /// SHA-256 content hash for change detection.
    ///
    /// Covers title + body + tags (not links/meta, which are structural).
    /// Used to detect actual content changes since yrs state vectors grow
    /// monotonically even on undo.
    pub fn content_hash(&self) -> String {
        let mat = self.materialize();
        let mut hasher = Sha256::new();
        hasher.update(mat.title.as_bytes());
        hasher.update(b"\0");
        hasher.update(mat.body.as_bytes());
        hasher.update(b"\0");
        for tag in &mat.tags {
            hasher.update(tag.as_bytes());
            hasher.update(b"\0");
        }
        hex::encode(hasher.finalize())
    }

    /// Access the underlying Doc.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }
}
