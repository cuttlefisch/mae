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
/// Provenance (`mae_kb::NodeSource`, as its serialized string).
///
/// @ai-caution: [kb-truth] Provenance MUST cross the wire. `NodeSource::Seed` is
/// the only enforced read-only mechanism for shipped content, and before this key
/// existed a shared node arrived at the peer re-stamped `Federation` — so a
/// read-only corpus became fully editable the moment it was shared (#710). That
/// is also the real reason ADR-104 D1 refuses to share system KBs at all: the
/// refusal is a workaround for this gap, not a policy in its own right.
const SOURCE_KEY: &str = "source";
/// Creation timestamp (unix seconds), stamped ONCE at construction.
///
/// @ai-caution: [kb-truth] Immutable by contract — there is deliberately no
/// setter. The Cozo row's `created_at` is written as `now` on EVERY insert, so
/// it records "last written" rather than "created", and a re-ingest destroys
/// node age outright; the one stored view that reads it (`view:backlog`) has
/// therefore been ordering by the wrong thing. A field living only in the
/// projection is also destroyed by the next rebuild (ADR-029), which is why this
/// belongs in the document rather than being patched in the row encoder.
const CREATED_KEY: &str = "created";

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
    /// Provenance, as its serialized string. `None` for a v1 document, and for a
    /// v2 document authored before `source` joined the schema.
    pub source: Option<String>,
    /// Creation timestamp (unix seconds). `None` for a document authored before
    /// the key existed.
    pub created_at: Option<i64>,
}

/// Seconds since the unix epoch, or 0 if the clock predates it.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
            // Stamped once, here, and never moved — node age is a fact about the
            // node, not about the last time something wrote it.
            Self::stamp_created_at(&root, &mut txn, unix_now());
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

    /// Creation timestamp (unix seconds). Absent ⇒ `None`, for a document
    /// authored before this key existed.
    pub fn created_at(&self) -> Option<i64> {
        self.scalar(CREATED_KEY).and_then(|s| s.parse::<i64>().ok())
    }

    /// Stamp the creation time if the document does not already carry one.
    ///
    /// Idempotent and one-way — there is no setter that can move it. An existing
    /// document gains a stamp on first construction-from-bytes rather than
    /// staying blank forever, but never a SECOND one.
    fn stamp_created_at(root: &yrs::MapRef, txn: &mut yrs::TransactionMut, now: i64) {
        use yrs::Map;
        if root.get(txn, CREATED_KEY).is_none() {
            root.insert(txn, CREATED_KEY, now.to_string());
        }
    }

    /// Provenance (`mae_kb::NodeSource` as its serialized string). Absent ⇒ `None`.
    ///
    /// Tolerant reader per ADR-093: a document authored before this key existed
    /// returns `None`, and the caller keeps whatever provenance it already had
    /// rather than having it blanked.
    pub fn source(&self) -> Option<String> {
        self.scalar(SOURCE_KEY)
    }

    /// Set the provenance. Returns the encoded update.
    #[must_use = "dropping this update silently prevents provenance from syncing, which is #710"]
    pub fn set_source(&mut self, source: Option<&str>) -> Vec<u8> {
        self.set_scalar(SOURCE_KEY, source)
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
            source: self.source(),
            created_at: self.created_at(),
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

    /// ADR-107: the hash a **rebirth** signs — covering everything the reborn
    /// document must reproduce.
    ///
    /// Deliberately NOT [`content_hash`](Self::content_hash), which covers only
    /// title + body + tags. That is the right scope for *change detection* (its
    /// stated job) and the wrong one for a rebirth: a rebirth discards the whole
    /// operation history, so anything outside the hash is unverifiable
    /// afterwards. Reusing it would have made ADR-107's content-identity gate
    /// pass while `kind`, `todo_state`, `priority`, `aliases`, `properties`,
    /// `source*` and links silently changed across the boundary.
    ///
    /// Two hashes for two questions, each named for its own (principle #8 is
    /// about one MECHANISM per question, not one function for two).
    pub fn rebirth_hash(&self) -> String {
        let mat = self.materialize();
        let mut h = Sha256::new();
        let mut field = |b: &[u8]| {
            h.update(b);
            h.update(b"\0");
        };
        field(b"maerebirth/v1");
        field(mat.id.as_bytes());
        field(mat.title.as_bytes());
        field(mat.body.as_bytes());
        // Ordered collections hash in order; unordered ones are sorted first, so
        // two peers that assembled the same set differently agree.
        for t in &mat.tags {
            field(t.as_bytes());
        }
        field(b"|links|");
        let mut links = mat.links.clone();
        links.sort();
        for l in &links {
            field(l.as_bytes());
        }
        field(b"|v2|");
        field(mat.kind.as_deref().unwrap_or("").as_bytes());
        field(mat.todo_state.as_deref().unwrap_or("").as_bytes());
        field(mat.priority.as_deref().unwrap_or("").as_bytes());
        field(mat.source.as_deref().unwrap_or("").as_bytes());
        field(
            mat.source_version
                .map(|v| v.to_string())
                .unwrap_or_default()
                .as_bytes(),
        );
        let mut aliases = mat.aliases.clone();
        aliases.sort();
        for a in &aliases {
            field(a.as_bytes());
        }
        field(b"|props|");
        let mut props: Vec<(&String, &String)> = mat.properties.iter().collect();
        props.sort();
        for (k, v) in props {
            field(k.as_bytes());
            field(v.as_bytes());
        }
        hex::encode(h.finalize())
    }

    /// ADR-107: re-emit this node as a **fresh single-client document** carrying
    /// the same materialized content, discarding its operation history.
    ///
    /// This is the growth bound. A yrs document grows monotonically in
    /// *operations* — deletion is a flag on the Item, and GC replaces deleted
    /// content while the Item and its delete-set entry remain — so a node edited
    /// daily for years accrues state nothing reclaims. Rebirth resets that to the
    /// cost of the content itself.
    ///
    /// **The caller must author a signed `Rebirth` op** carrying
    /// [`rebirth_hash`](Self::rebirth_hash) of the RESULT, and peers must adopt
    /// via that op rather than merging. Merging is the failure mode: yrs would
    /// happily merge a stale lineage back in and resurrect the very history this
    /// discarded.
    ///
    /// Returns the reborn document. The old one is unchanged — the caller
    /// replaces it only once the manifest hash advances (ADR-107 D3), so there is
    /// no window in which a peer sees neither.
    pub fn reborn(&self, client_id: u64) -> Self {
        let mat = self.materialize();
        let mut doc =
            Self::new_with_client_id(&mat.id, &mat.title, &mat.body, &mat.tags, client_id);
        // Every v2 field, or the reborn node quietly loses metadata across the
        // boundary -- the same defect #656 had on the fresh-lineage path, which
        // is what makes this worth writing out rather than trusting `new`.
        let _ = doc.set_kind(mat.kind.as_deref());
        let _ = doc.set_todo_state(mat.todo_state.as_deref());
        let _ = doc.set_priority(mat.priority.as_deref());
        let _ = doc.set_source(mat.source.as_deref());
        let _ = doc.set_source_version(mat.source_version);
        let _ = doc.set_aliases(&mat.aliases);
        let _ = doc.set_properties(&mat.properties);
        for l in &mat.links {
            let _ = doc.add_link(l);
        }
        doc
    }

    /// Access the underlying Doc.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }
}

/// ADR-107's verification gates for node rebirth.
///
/// These are the four the ADR states *"now so they are not negotiated later"*.
/// Gate 1 (growth is bounded) is the one nothing measured before: the ADR notes
/// that *"no test asserts document growth at all beyond the two added with
/// #744"*.
#[cfg(test)]
mod rebirth_tests {
    use super::*;

    /// Edit a node `n` times, so its op log accumulates.
    fn edit_n_times(doc: &mut KbNodeDoc, n: usize) {
        for i in 0..n {
            let _ = doc.set_title(&format!("Title revision {i}"));
            let _ = doc.set_body(&format!("Body revision {i} — {}", "x".repeat(40)));
        }
    }

    fn v2_doc() -> KbNodeDoc {
        let mut d = KbNodeDoc::new("note:a", "Original", "original body", &["alpha".into()]);
        let _ = d.set_kind(Some("task"));
        let _ = d.set_todo_state(Some("TODO"));
        let _ = d.set_priority(Some("A"));
        let _ = d.set_source(Some("user_org"));
        let _ = d.set_source_version(Some(3));
        let _ = d.set_aliases(&["a-prime".to_string()]);
        let mut props = std::collections::HashMap::new();
        props.insert("role".to_string(), "owner".to_string());
        let _ = d.set_properties(&props);
        let _ = d.add_link("note:b");
        d
    }

    /// **ADR-107 gate 1: growth is actually bounded.**
    ///
    /// *"A node edited N times, reborn, then edited N times again must not exceed
    /// a fixed multiple of its materialized size."*
    ///
    /// This is the whole claim of the ADR. Without it, rebirth is an assertion.
    ///
    /// Measured on this fixture (200 title+body edits, a v2 node with links,
    /// aliases and properties): **7,128 B grown → 383 B reborn**, an 18.6x
    /// reduction. The second cycle regrows to 2,777 B and returns to **exactly
    /// 383 B** — identical, which is what distinguishes a BOUND from a saving.
    /// The thresholds below are deliberately looser than those figures so the
    /// test pins the property rather than the constants.
    #[test]
    fn rebirth_bounds_growth_rather_than_merely_slowing_it() {
        let mut doc = v2_doc();
        edit_n_times(&mut doc, 200);
        let grown = doc.encode_state().len();

        let reborn = doc.reborn(2);
        let after_rebirth = reborn.encode_state().len();

        assert!(
            after_rebirth < grown / 4,
            "a reborn document must shed the operation history, not carry it: \
             {grown} B grown vs {after_rebirth} B reborn"
        );

        // ...and the bound HOLDS across a second cycle, which is what makes it a
        // bound rather than a one-off saving.
        let mut again = reborn;
        edit_n_times(&mut again, 200);
        let regrown = again.encode_state().len();
        let reborn_again = again.reborn(3).encode_state().len();

        assert!(
            reborn_again < after_rebirth * 2,
            "the second rebirth must return to roughly the same floor \
             ({after_rebirth} B then {reborn_again} B), else growth is merely \
             slowed: {regrown} B before the second rebirth"
        );
    }

    /// **ADR-107 gate 4: content identity**, verified by hash rather than by
    /// inspection — the ADR says so explicitly.
    #[test]
    fn a_reborn_node_is_content_identical_to_its_predecessor() {
        let mut doc = v2_doc();
        edit_n_times(&mut doc, 20);

        let before = doc.rebirth_hash();
        let reborn = doc.reborn(2);

        assert_eq!(
            reborn.rebirth_hash(),
            before,
            "rebirth must preserve content exactly -- it discards HISTORY, not data"
        );
    }

    /// Every v2 field survives, individually asserted.
    ///
    /// The hash test above would catch a loss, but not say WHICH field -- and
    /// #656 was precisely a fresh-lineage path silently dropping v2 fields, so
    /// this names them.
    #[test]
    fn rebirth_preserves_every_v2_field_not_just_the_text() {
        let doc = v2_doc();
        let reborn = doc.reborn(2);
        let m = reborn.materialize();

        assert_eq!(m.title, "Original");
        assert_eq!(m.body, "original body");
        assert_eq!(m.tags, vec!["alpha".to_string()]);
        assert_eq!(m.kind.as_deref(), Some("task"));
        assert_eq!(m.todo_state.as_deref(), Some("TODO"));
        assert_eq!(m.priority.as_deref(), Some("A"));
        assert_eq!(m.source.as_deref(), Some("user_org"));
        assert_eq!(m.source_version, Some(3));
        assert_eq!(m.aliases, vec!["a-prime".to_string()]);
        assert_eq!(m.properties.get("role").map(String::as_str), Some("owner"));
        assert_eq!(m.links, vec!["note:b".to_string()]);
        assert_eq!(
            reborn.schema_version(),
            2,
            "a reborn v2 node must still be v2"
        );
    }

    /// The rebirth hash must cover MORE than `content_hash` does.
    ///
    /// If it did not, the content-identity gate would pass while metadata
    /// changed across a boundary that destroys the history needed to notice.
    #[test]
    fn the_rebirth_hash_notices_metadata_that_content_hash_ignores() {
        let base = v2_doc();
        let mut changed = v2_doc();
        let _ = changed.set_todo_state(Some("DONE"));

        assert_eq!(
            base.content_hash(),
            changed.content_hash(),
            "content_hash covers title+body+tags only -- unchanged here, which is \
             exactly why it is the wrong hash for a rebirth"
        );
        assert_ne!(
            base.rebirth_hash(),
            changed.rebirth_hash(),
            "the rebirth hash MUST notice a todo_state change"
        );
    }

    /// The reborn document is genuinely a fresh lineage, not a copy carrying the
    /// old client id -- otherwise two peers could derive colliding ClientIDs,
    /// which Yjs documents as permanent unrecoverable corruption.
    #[test]
    fn a_reborn_document_is_a_fresh_lineage() {
        let doc = v2_doc();
        let reborn = doc.reborn(4242);
        // Compare against a doc created with the same id rather than a bare
        // integer -- `ClientID` is yrs's own type and its representation is not
        // this test's business.
        let expected = KbNodeDoc::new_with_client_id("note:z", "T", "B", &[], 4242);
        assert_eq!(reborn.doc().client_id(), expected.doc().client_id());
        assert_ne!(
            doc.doc().client_id(),
            reborn.doc().client_id(),
            "the point of rebirth is a NEW single-client document"
        );
    }

    /// A hash is order-insensitive for unordered collections, so two peers that
    /// assembled the same tags/aliases/properties in different orders agree.
    #[test]
    fn the_rebirth_hash_is_stable_across_collection_ordering() {
        let mut a = KbNodeDoc::new("note:x", "T", "B", &[]);
        let _ = a.set_aliases(&["one".into(), "two".into()]);
        let mut b = KbNodeDoc::new("note:x", "T", "B", &[]);
        let _ = b.set_aliases(&["two".into(), "one".into()]);

        assert_eq!(
            a.rebirth_hash(),
            b.rebirth_hash(),
            "alias ORDER must not change the hash -- otherwise two peers holding \
             the same content disagree about whether a rebirth is current"
        );
    }
}
