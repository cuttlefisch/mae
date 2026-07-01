//! KbNodeDoc: yrs-backed KB node with YMap schema.
//!
//! All yrs Doc instances use UTF-16 offset kind (via `text::new_doc()`) for
//! consistency with the Yjs standard. See the CRDT UTF-16 fix (92a20b8).

use sha2::{Digest, Sha256};
use yrs::{
    updates::decoder::Decode, updates::encoder::Encode, Array, ArrayPrelim, Doc, GetString, Map,
    MapPrelim, MapRef, Out, ReadTxn, Text, TextPrelim, Transact,
};

use crate::membership::{MembershipAction, MembershipOp, SignedMembershipOp};
use crate::text::{new_doc, new_doc_with_client_id};
use crate::SyncError;

const ID_KEY: &str = "id";
const TITLE_KEY: &str = "title";
const BODY_KEY: &str = "body";
const TAGS_KEY: &str = "tags";
const LINKS_KEY: &str = "links";
const META_KEY: &str = "meta";

/// Derive a stable yrs `client_id` for KB CRDT edits from a peer's durable collab
/// identity `fingerprint` + its per-KB authorization `epoch` (ADR-020 B-16,
/// ADR-023). FNV-1a over the fingerprint then the epoch, folded into the **53-bit**
/// range yrs permits (B-17 — a full u64 panics in debug / silently truncates in
/// release), never `0`/`1`. A role change bumps the epoch → a *different,
/// unrelated* client_id, so the daemon can fence a member's pre-grant (stale-epoch)
/// ops. Lives here (mae-sync) so both the editor and the daemon derive identically.
pub fn derive_kb_client_id(fingerprint: &str, epoch: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in fingerprint.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for b in epoch.to_le_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let folded = (h ^ (h >> 53)) & ((1u64 << 53) - 1);
    if folded == 0 || folded == 1 {
        2
    } else {
        folded
    }
}

/// A fresh, **unpredictable** authorization-epoch token (#72). Issued by the
/// daemon (the sole author of membership mutations) when a member's epoch must
/// advance — a role change, or a re-grant of a previously-removed member. Unlike
/// a `prev+1` counter, a malicious client cannot precompute
/// `derive_kb_client_id(fp, future_epoch)` and back-date viewer-era ops under the
/// future editor client_id (the ADR-023 "pre-rotation" attack). Non-zero so it
/// never collides with the epoch-0 "fresh grant" sentinel.
pub fn fresh_epoch_token() -> u64 {
    // `rand::random` is the version-stable top-level API (avoids a trait import
    // whose path shifts between rand 0.9 and 0.10). Re-roll the (2^-64) zero so the
    // token is never the epoch-0 sentinel — without special-casing it to a fixed
    // value (which would bias the distribution toward that value).
    loop {
        let t = rand::random::<u64>();
        if t != 0 {
            return t;
        }
    }
}

/// True if `candidate_sv` carries operations not covered by `base_sv` — i.e.
/// some client's clock in `candidate_sv` exceeds what `base_sv` has seen.
///
/// Both args are v1-encoded yrs state vectors. This is the format-independent
/// primitive behind ADR-022 reconcile decisions: "is the remote ahead of me?"
/// (`sv_has_ops_beyond(remote_sv, my_sv)`) and "am I ahead of the remote?"
/// (`sv_has_ops_beyond(my_sv, remote_sv)`). Avoids the trap that an `encode_diff`
/// against a fully-covering SV is still a non-empty (`[0,0]`) byte sequence.
pub fn sv_has_ops_beyond(candidate_sv: &[u8], base_sv: &[u8]) -> Result<bool, SyncError> {
    let candidate = yrs::StateVector::decode_v1(candidate_sv)
        .map_err(|e| SyncError::Encoding(e.to_string()))?;
    let base =
        yrs::StateVector::decode_v1(base_sv).map_err(|e| SyncError::Encoding(e.to_string()))?;
    for (client, &clock) in candidate.iter() {
        if base.get(client) < clock {
            return Ok(true);
        }
    }
    Ok(false)
}

/// True if two v1-encoded state vectors share **no** client id — i.e. the two
/// documents were constructed on entirely independent lineages.
///
/// This is the order-independent ADR-022 divergence signal: any healthy collab
/// pair shares at least the owner's lineage client (the joiner adopted it on
/// first join), so a *disjoint* client set means the nodes were built from
/// scratch with the same id but incompatible lineages (the B-14 condition) —
/// regardless of which side happens to win the YMap last-writer-wins.
pub fn sv_clients_disjoint(a_sv: &[u8], b_sv: &[u8]) -> Result<bool, SyncError> {
    let a = yrs::StateVector::decode_v1(a_sv).map_err(|e| SyncError::Encoding(e.to_string()))?;
    let b = yrs::StateVector::decode_v1(b_sv).map_err(|e| SyncError::Encoding(e.to_string()))?;
    for (client, _) in a.iter() {
        if b.contains_client(client) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// ADR-023: the set of client_ids that authored operations in `update` that are
/// **not yet covered by `base_state`** — i.e. the "new" ops a peer is contributing
/// on top of the daemon's authoritative node STATE. The daemon fences with this:
/// every new-op author must equal the member's current-epoch client_id
/// (`derive_kb_client_id(fp, epoch_now)`); a stale-epoch author means a pre-grant
/// (or otherwise unauthorized) lineage and the write is rejected (`rebase required`).
///
/// Takes the full authoritative **state** (not merely its state vector) because a
/// naive `Update::state_vector()` comparison has a blind spot (B-20): an op that is
/// a *contiguous-clock continuation* of a client already present in the base does
/// **not** appear in the incoming update's own state vector, so a member who keeps
/// authoring under a still-canonical client_id (e.g. they never rotated off it after
/// a demotion) could append post-demotion edits and slip the fence. Detecting that
/// requires integrating the update against the real state and observing which
/// clients' clocks actually advanced. We do exactly that, then UNION the legacy
/// `Update::state_vector()` signal so we never fence *fewer* ops than before
/// (independent/divergent lineages whose ops can't integrate into the base stay
/// caught).
pub fn update_new_op_authors(update: &[u8], base_state: &[u8]) -> Result<Vec<u64>, SyncError> {
    let mut authors = std::collections::BTreeSet::new();

    // Primary signal — apply-and-diff against the authoritative state. Integrating
    // the update reveals contiguous continuations of an already-known client (the
    // B-20 vector) that the update's own SV omits, as well as fresh clients whose
    // ops depend only on the base.
    let mut doc = KbNodeDoc::from_bytes(base_state)?;
    let before = yrs::StateVector::decode_v1(&doc.state_vector())
        .map_err(|e| SyncError::Encoding(e.to_string()))?;
    doc.apply_update(update)?;
    let after = yrs::StateVector::decode_v1(&doc.state_vector())
        .map_err(|e| SyncError::Encoding(e.to_string()))?;
    for (client, &clock) in after.iter() {
        if before.get(client) < clock {
            authors.insert(client.get());
        }
    }

    // Defense in depth — the legacy signal. Catches ops from an independent lineage
    // that do NOT integrate into the base (they remain pending, so apply-and-diff
    // wouldn't advance the SV), preserving the pre-B-20 fencing for that case.
    let upd = yrs::Update::decode_v1(update).map_err(|e| SyncError::Encoding(e.to_string()))?;
    for (client, &clock) in upd.state_vector().iter() {
        if before.get(client) < clock {
            authors.insert(client.get());
        }
    }

    Ok(authors.into_iter().collect())
}

/// Materialized content from a KbNodeDoc — all fields extracted for FTS rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedNode {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
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
    pub fn set_title(&mut self, title: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YText(text)) = root.get(&txn, TITLE_KEY) {
            let len = text.get_string(&txn).encode_utf16().count() as u32;
            if len > 0 {
                text.remove_range(&mut txn, 0, len);
            }
            text.insert(&mut txn, 0, title);
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
    pub fn set_body(&mut self, body: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YText(text)) = root.get(&txn, BODY_KEY) {
            let len = text.get_string(&txn).encode_utf16().count() as u32;
            if len > 0 {
                text.remove_range(&mut txn, 0, len);
            }
            text.insert(&mut txn, 0, body);
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

    /// Replace ALL tags with `tags` (clear + re-insert the `YArray`). Returns the
    /// encoded update. This is the setter `upsert_with_crdt` needs for a wholesale
    /// tag edit (e.g. `kb_update` with a new tags list) to enter the CRDT and
    /// broadcast a delta — B-18: previously only `set_title`/`set_body` were wired,
    /// so tag changes after node creation never synced (peer apply was a no-op).
    /// Mirrors `set_title`'s clear-then-insert so the change chains on the lineage.
    pub fn set_tags(&mut self, tags: &[String]) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(arr)) = root.get(&txn, TAGS_KEY) {
            let len = arr.len(&txn);
            if len > 0 {
                arr.remove_range(&mut txn, 0, len);
            }
            for tag in tags {
                arr.push_back(&mut txn, tag.as_str());
            }
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

// --- KbCollectionDoc: manifest of nodes in a shared KB ---

const COLLECTION_MAP: &str = "collection";
const COLL_NAME_KEY: &str = "name";
const COLL_CREATOR_KEY: &str = "creator"; // legacy display label (never authoritative)
const COLL_NODES_KEY: &str = "nodes";
const COLL_MEMBERS_KEY: &str = "members"; // legacy YArray<label>, read-only after ADR-018
                                          // ADR-018 identity-anchored schema (v2):
const COLL_SCHEMA_KEY: &str = "schema";
const COLL_OWNER_KEY: &str = "owner"; // owner principal (key fingerprint)
const COLL_MEMBER_ROLES_KEY: &str = "member_roles"; // YMap<fingerprint -> {role,label}>
const COLL_POLICY_KEY: &str = "join_policy"; // restrictive|invite|permissive
const COLL_PENDING_KEY: &str = "pending"; // YMap<fingerprint -> {label,requested_at}>
const COLL_RETIRED_KEY: &str = "retired"; // YMap<fingerprint -> last epoch> (#72 tombstone)
const COLL_TRANSPORT_POLICY_KEY: &str = "transport_policy"; // hub|p2p|both (absent ⇒ hub)
const COLL_ENCRYPTION_KEY: &str = "encryption"; // ADR-037: none|e2e (absent ⇒ none)
/// ADR-026 signed membership op-log: `YMap<chain_hash -> op record>` — the
/// append-only, CRDT *set* of signed membership ops (keyed by each op's
/// `chain_hash` so concurrent appends converge). Validity is *derived* by every
/// peer replaying this log (`derive_valid_members`), never read as a trusted
/// verdict. This is the v3 source of truth; the v2 `member_roles` LWW map remains
/// as a migration read-path until the daemon switches the gate over (Phase 2b-6).
const COLL_OPLOG_KEY: &str = "membership_oplog";
// Sub-keys of one op record (a YMap value under COLL_OPLOG_KEY, keyed by chain_hash).
const OP_KBID_KEY: &str = "kb_id"; // signed kb_id (self-contained verification)
const OP_ACTION_KEY: &str = "action"; // admit|remove|set_role|revoke
const OP_SUBJECT_KEY: &str = "subject"; // principal acted on
const OP_ROLE_KEY: &str = "role"; // granted role ("" if none)
const OP_CAN_INVITE_KEY: &str = "can_invite"; // "1"|"0"
const OP_AUTHOR_KEY: &str = "author"; // issuer principal (= signer fingerprint)
const OP_ISSUED_KEY: &str = "issued_at"; // unix seconds (decimal)
const OP_EXPIRES_KEY: &str = "expires_at"; // unix seconds (decimal); "" = no timebox
const OP_EPOCH_KEY: &str = "epoch"; // ADR-023 authorization epoch assigned to subject (decimal)
const OP_PREV_KEY: &str = "prev_hash"; // chain_hash of the author's view-head ("" = genesis)
const OP_SIG_KEY: &str = "sig"; // hex(64-byte Ed25519 signature)
const OP_PUBKEY_KEY: &str = "author_pubkey"; // hex(32-byte Ed25519 public key)
const OP_WRAPPED_KEY: &str = "wrapped_key"; // ADR-037: hex(content key wrapped to subject); absent = none
const OP_NEW_PUBKEY_KEY: &str = "new_pubkey"; // ADR-040: hex(successor Ed25519 pubkey) on a Rebind; absent = none
const OP_NEW_WRAP_PUBKEY_KEY: &str = "new_wrap_pubkey"; // ADR-040/I1: hex(successor X25519 wrap pubkey) on a Rebind
const OP_RECOVERY_PUBKEY_KEY: &str = "recovery_pubkey"; // ADR-040 §Recovery: hex(recovery Ed25519 pubkey) on a RegisterRecoveryKey
const MEMBER_ROLE_KEY: &str = "role";
const MEMBER_LABEL_KEY: &str = "label";
/// ADR-023: per-member monotonic authorization epoch, bumped by the daemon on
/// every role change for that member. Drives the epoch-fenced rebase: the KB
/// client_id a member authors under is `derive_kb_client_id(fp, epoch)`, so a
/// role change rotates it and the daemon can fence pre-grant (stale-epoch) ops.
/// Stored as a decimal string (mirrors the role/label string fields).
const MEMBER_EPOCH_KEY: &str = "epoch";
const MEMBER_PUBKEY_KEY: &str = "pubkey"; // hex Ed25519, for E2e re-wrap on rotation (ADR-038)
                                          // ADR-041 (#158 I1): the member's PUBLISHED X25519 wrap key (hex), distinct from the
                                          // Ed25519 identity. The owner wraps the content key to THIS, not to a key derived from
                                          // the ed25519 pubkey (which is impossible). Stored alongside the ed25519 pubkey.
const MEMBER_WRAP_PUBKEY_KEY: &str = "wrap_pubkey";
const PENDING_AT_KEY: &str = "requested_at";
const PENDING_PUBKEY_KEY: &str = "pubkey"; // hex Ed25519 (ADR-038, optional)
const PENDING_WRAP_PUBKEY_KEY: &str = "wrap_pubkey"; // hex X25519 wrap key (ADR-041 / I1)

/// Collection schema version. v2 = ADR-018 (principal-anchored owner/roles/policy).
pub const SCHEMA_VERSION: u32 = 2;

/// A KB role. Hierarchical RBAC (NIST INCITS 359): `owner ⊇ editor ⊇ viewer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Owner,
    Editor,
    Viewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Editor => "editor",
            Role::Viewer => "viewer",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "owner" => Some(Role::Owner),
            "editor" => Some(Role::Editor),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
    fn rank(self) -> u8 {
        match self {
            Role::Viewer => 0,
            Role::Editor => 1,
            Role::Owner => 2,
        }
    }
    /// Role inheritance: a senior role holds all junior permissions.
    pub fn includes(self, other: Role) -> bool {
        self.rank() >= other.rank()
    }
}

/// Per-KB join policy (maps to Drive sharing modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum JoinPolicy {
    /// Default-deny: only owner + explicitly added members.
    Restrictive,
    /// Non-members' joins become pending → owner approves (default).
    #[default]
    Invite,
    /// Any authenticated peer auto-joins as viewer.
    Permissive,
}

impl JoinPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            JoinPolicy::Restrictive => "restrictive",
            JoinPolicy::Invite => "invite",
            JoinPolicy::Permissive => "permissive",
        }
    }
    pub fn parse(s: &str) -> Option<JoinPolicy> {
        match s {
            "restrictive" => Some(JoinPolicy::Restrictive),
            "invite" => Some(JoinPolicy::Invite),
            "permissive" => Some(JoinPolicy::Permissive),
            _ => None,
        }
    }
}

/// The transport a connection arrived on — the input to per-KB transport-policy
/// enforcement (ADR-018/025). The hub TCP listener and the iroh mesh tag their
/// connections so `kb_access` can apply [`TransportPolicy`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// The v0.14 hub TCP listener.
    Hub,
    /// The iroh P2P mesh.
    P2p,
}

/// Which transport(s) a shared KB is exposed over (ADR-018/025). **Absent ⇒ Hub**
/// (conservative: enabling the mesh never silently exposes an existing hub share;
/// a KB is mesh-reachable only once it is explicitly p2p-shared). Local-only KBs
/// have no collection doc, so they carry no policy and are never reachable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TransportPolicy {
    /// Hub TCP listener only (the conservative default).
    #[default]
    Hub,
    /// P2P mesh only.
    P2p,
    /// Both transports.
    Both,
}

impl TransportPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportPolicy::Hub => "hub",
            TransportPolicy::P2p => "p2p",
            TransportPolicy::Both => "both",
        }
    }
    pub fn parse(s: &str) -> Option<TransportPolicy> {
        match s {
            "hub" => Some(TransportPolicy::Hub),
            "p2p" => Some(TransportPolicy::P2p),
            "both" => Some(TransportPolicy::Both),
            _ => None,
        }
    }
    /// Whether this policy exposes the KB over `transport`.
    pub fn allows(self, transport: Transport) -> bool {
        matches!(
            (self, transport),
            (TransportPolicy::Both, _)
                | (TransportPolicy::Hub, Transport::Hub)
                | (TransportPolicy::P2p, Transport::P2p)
        )
    }
    /// Widen this policy to also expose `transport` (idempotent). Mixing the two
    /// transports yields `Both`. Used by `kb-share` / `kb-share-p2p`.
    pub fn with(self, transport: Transport) -> TransportPolicy {
        match (self, transport) {
            (TransportPolicy::Both, _)
            | (TransportPolicy::Hub, Transport::Hub)
            | (TransportPolicy::P2p, Transport::P2p) => self,
            _ => TransportPolicy::Both,
        }
    }
    /// The union of two exposure policies (`Hub ∪ P2p = Both`). Used to widen a
    /// KB's exposure when it is (re-)shared over an additional transport.
    pub fn union(self, other: TransportPolicy) -> TransportPolicy {
        use TransportPolicy::*;
        match (self, other) {
            (Both, _) | (_, Both) => Both,
            (Hub, Hub) => Hub,
            (P2p, P2p) => P2p,
            _ => Both,
        }
    }
}

/// ADR-037 per-KB content-encryption mode. `None` (the default, and what every
/// absent flag reads as) keeps v0.14 KBs plaintext + unchanged; `E2e` marks the KB's
/// content ops as encrypted under a per-KB content key distributed via the membership
/// op-log. The opt-in is per-KB and on the collection doc, mirroring `TransportPolicy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Encryption {
    /// Plaintext content ops (default; absent flag).
    #[default]
    None,
    /// End-to-end encrypted content ops (per-KB content key, ADR-037).
    E2e,
}

impl Encryption {
    pub fn as_str(self) -> &'static str {
        match self {
            Encryption::None => "none",
            Encryption::E2e => "e2e",
        }
    }
    pub fn parse(s: &str) -> Option<Encryption> {
        match s {
            "none" => Some(Encryption::None),
            "e2e" => Some(Encryption::E2e),
            _ => None,
        }
    }
}

/// A KB member: the principal (key fingerprint), its role, and a display label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub fingerprint: String,
    pub role: Role,
    pub label: String,
}

/// A pending join request (invite policy).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRequest {
    pub fingerprint: String,
    pub label: String,
    pub requested_at: String,
    /// ADR-038: the joiner's Ed25519 public key, captured by the daemon from the
    /// authenticated session at `kb/join`. Rides the `kbc:` broadcast so the OWNER can
    /// `wrap_to_member` the content key on approval (the owner has only the fingerprint
    /// otherwise). `None` for a pre-ADR-038 pending record (backward-compatible).
    pub pubkey: Option<[u8; 32]>,
    /// ADR-041 (#158 I1): the joiner's PUBLISHED X25519 wrap key (the owner wraps the
    /// content key to THIS, not the ed25519 key). The joiner sends it on `kb/join` (the
    /// daemon can't derive it). `None` for a pre-ADR-041 record.
    pub wrap_pubkey: Option<[u8; 32]>,
}

/// A KB collection manifest represented as a yrs document.
///
/// Schema:
/// - Root YMap "collection" contains:
///   - name (String): KB display name
///   - creator (String): creator's user name
///   - nodes (YMap<node_id -> String(title)>): node manifest
///   - members (YArray<String>): member user names
///
/// The collection doc is stored on the server as `kbc:{kb_id}`.
pub struct KbCollectionDoc {
    doc: Doc,
}

impl KbCollectionDoc {
    /// Create a new collection document.
    pub fn new(name: &str, creator: &str) -> Self {
        let doc = new_doc();
        {
            let root = doc.get_or_insert_map(COLLECTION_MAP);
            let mut txn = doc.transact_mut();
            root.insert(&mut txn, COLL_NAME_KEY, name);
            root.insert(&mut txn, COLL_CREATOR_KEY, creator);
            root.insert(&mut txn, COLL_NODES_KEY, MapPrelim::default());
            let members = root.insert(&mut txn, COLL_MEMBERS_KEY, ArrayPrelim::default());
            members.push_back(&mut txn, creator);
        }
        Self { doc }
    }

    /// Create a new collection document with a specific client ID.
    pub fn new_with_client_id(name: &str, creator: &str, client_id: u64) -> Self {
        let doc = new_doc_with_client_id(client_id);
        {
            let root = doc.get_or_insert_map(COLLECTION_MAP);
            let mut txn = doc.transact_mut();
            root.insert(&mut txn, COLL_NAME_KEY, name);
            root.insert(&mut txn, COLL_CREATOR_KEY, creator);
            root.insert(&mut txn, COLL_NODES_KEY, MapPrelim::default());
            let members = root.insert(&mut txn, COLL_MEMBERS_KEY, ArrayPrelim::default());
            members.push_back(&mut txn, creator);
        }
        Self { doc }
    }

    /// Load from encoded bytes.
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

    /// Encode full state.
    pub fn encode_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
    }

    /// State vector for sync.
    pub fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    /// Apply a remote update.
    pub fn apply_update(&mut self, update: &[u8]) -> Result<(), SyncError> {
        let update =
            yrs::Update::decode_v1(update).map_err(|e| SyncError::Encoding(e.to_string()))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|e| SyncError::Encoding(e.to_string()))?;
        Ok(())
    }

    /// Get KB name.
    pub fn name(&self) -> String {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_NAME_KEY)
            .map(|v| v.to_string(&txn))
            .unwrap_or_default()
    }

    /// Get creator name.
    pub fn creator(&self) -> String {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_CREATOR_KEY)
            .map(|v| v.to_string(&txn))
            .unwrap_or_default()
    }

    /// Add a node to the collection manifest. Returns encoded update.
    pub fn add_node(&mut self, node_id: &str, title: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(nodes)) = root.get(&txn, COLL_NODES_KEY) {
            nodes.insert(&mut txn, node_id, title);
        }
        txn.encode_update_v1()
    }

    /// #156 F5: blank every cleartext node title in the manifest in ONE transaction —
    /// used when E2e is enabled on an EXISTING KB so the key-blind daemon stops holding
    /// plaintext titles (the real title lives encrypted in the node op-set; the manifest
    /// only needs the `node_id`). Returns the encoded delta, or an **empty `Vec`** when
    /// there was nothing to blank (no nodes, or all titles already empty) — idempotent.
    pub fn blank_node_titles_delta(&mut self) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        let Some(Out::YMap(nodes)) = root.get(&txn, COLL_NODES_KEY) else {
            return Vec::new();
        };
        let mut with_titles: Vec<String> = Vec::new();
        for (k, v) in nodes.iter(&txn) {
            if !v.to_string(&txn).is_empty() {
                with_titles.push(k.to_string());
            }
        }
        if with_titles.is_empty() {
            return Vec::new();
        }
        for id in &with_titles {
            nodes.insert(&mut txn, id.as_str(), "");
        }
        txn.encode_update_v1()
    }

    /// Remove a node from the collection manifest. Returns encoded update.
    pub fn remove_node(&mut self, node_id: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(nodes)) = root.get(&txn, COLL_NODES_KEY) {
            nodes.remove(&mut txn, node_id);
        }
        txn.encode_update_v1()
    }

    /// List all nodes in the collection: (node_id, title) pairs.
    pub fn list_nodes(&self) -> Vec<(String, String)> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_NODES_KEY) {
            Some(Out::YMap(nodes)) => nodes
                .iter(&txn)
                .map(|(k, v)| (k.to_string(), v.to_string(&txn)))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Get number of nodes in the collection.
    pub fn node_count(&self) -> u32 {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_NODES_KEY) {
            Some(Out::YMap(nodes)) => nodes.len(&txn),
            _ => 0,
        }
    }

    /// Re-stamp the authoritative creator and ensure they are a member.
    /// Used by the daemon to bind a shared collection to the AUTHENTICATED peer
    /// identity (ADR-017 strict binding), overriding the client-supplied creator.
    /// Returns the encoded update.
    pub fn set_creator(&mut self, creator: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_CREATOR_KEY, creator);
        if let Some(Out::YArray(members)) = root.get(&txn, COLL_MEMBERS_KEY) {
            let already = members.iter(&txn).any(|v| v.to_string(&txn) == creator);
            if !already {
                members.push_back(&mut txn, creator);
            }
        }
        txn.encode_update_v1()
    }

    /// Add a member to the collection. Returns encoded update.
    pub fn add_member(&mut self, user_name: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(members)) = root.get(&txn, COLL_MEMBERS_KEY) {
            // Check for duplicates
            let already = members.iter(&txn).any(|v| v.to_string(&txn) == user_name);
            if !already {
                members.push_back(&mut txn, user_name);
            }
        }
        txn.encode_update_v1()
    }

    /// Remove a member from the collection. Returns encoded update.
    pub fn remove_member(&mut self, user_name: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(members)) = root.get(&txn, COLL_MEMBERS_KEY) {
            let idx = members
                .iter(&txn)
                .position(|v| v.to_string(&txn) == user_name);
            if let Some(idx) = idx {
                members.remove(&mut txn, idx as u32);
            }
        }
        txn.encode_update_v1()
    }

    /// List all members.
    pub fn members(&self) -> Vec<String> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_MEMBERS_KEY) {
            Some(Out::YArray(arr)) => arr.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        }
    }

    // --- ADR-018: identity-anchored owner / roles / join-policy / pending ---

    /// Create a v2 collection owned by `owner_principal` (a key fingerprint), with
    /// `owner_label` for display. Seeds schema=2, owner, the owner member entry
    /// (role=owner), join_policy=invite, an empty pending map, and legacy
    /// `creator`/`members` for back-compat reads. An empty owner principal is
    /// tolerated (the daemon stamps the real owner from the verified cert).
    pub fn new_owned(name: &str, owner_principal: &str, owner_label: &str) -> Self {
        Self::new_owned_with(
            name,
            owner_principal,
            owner_label,
            None,
            JoinPolicy::default(),
        )
    }

    /// Like `new_owned` but with an explicit client id and join policy.
    pub fn new_owned_with(
        name: &str,
        owner_principal: &str,
        owner_label: &str,
        client_id: Option<u64>,
        policy: JoinPolicy,
    ) -> Self {
        let doc = match client_id {
            Some(id) => new_doc_with_client_id(id),
            None => new_doc(),
        };
        {
            let root = doc.get_or_insert_map(COLLECTION_MAP);
            let mut txn = doc.transact_mut();
            root.insert(&mut txn, COLL_NAME_KEY, name);
            root.insert(&mut txn, COLL_SCHEMA_KEY, SCHEMA_VERSION as i64);
            root.insert(&mut txn, COLL_OWNER_KEY, owner_principal);
            root.insert(&mut txn, COLL_CREATOR_KEY, owner_label); // legacy display
            root.insert(&mut txn, COLL_NODES_KEY, MapPrelim::default());
            root.insert(&mut txn, COLL_POLICY_KEY, policy.as_str());
            root.insert(&mut txn, COLL_PENDING_KEY, MapPrelim::default());
            let m = root.insert(&mut txn, COLL_MEMBER_ROLES_KEY, MapPrelim::default());
            if !owner_principal.is_empty() {
                let entry = m.insert(&mut txn, owner_principal, MapPrelim::default());
                entry.insert(&mut txn, MEMBER_ROLE_KEY, Role::Owner.as_str());
                entry.insert(&mut txn, MEMBER_LABEL_KEY, owner_label);
            }
            // legacy members array (read-only after migration)
            let legacy = root.insert(&mut txn, COLL_MEMBERS_KEY, ArrayPrelim::default());
            legacy.push_back(&mut txn, owner_label);
        }
        Self { doc }
    }

    /// Schema version (0 = legacy v1, absent the schema key).
    pub fn schema_version(&self) -> u32 {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_SCHEMA_KEY)
            .map(|v| v.to_string(&txn).parse::<u32>().unwrap_or(0))
            .unwrap_or(0)
    }

    /// Owner principal (key fingerprint). Empty if unset.
    pub fn owner(&self) -> String {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_OWNER_KEY)
            .map(|v| v.to_string(&txn))
            .unwrap_or_default()
    }

    /// Owner display label (legacy `creator` field).
    pub fn owner_label(&self) -> String {
        self.creator()
    }

    /// The role of `principal` (key fingerprint), if it is a member.
    pub fn role_of(&self, principal: &str) -> Option<Role> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            if let Some(Out::YMap(entry)) = m.get(&txn, principal) {
                return entry
                    .get(&txn, MEMBER_ROLE_KEY)
                    .map(|r| r.to_string(&txn))
                    .and_then(|s| Role::parse(&s));
            }
        }
        None
    }

    /// All members with their roles (the ReBAC tuple set for this KB).
    pub fn member_roles(&self) -> Vec<Member> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        let mut out = Vec::new();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            for (fp, v) in m.iter(&txn) {
                if let Out::YMap(entry) = v {
                    let role = entry
                        .get(&txn, MEMBER_ROLE_KEY)
                        .map(|r| r.to_string(&txn))
                        .and_then(|s| Role::parse(&s))
                        .unwrap_or(Role::Viewer);
                    let label = entry
                        .get(&txn, MEMBER_LABEL_KEY)
                        .map(|l| l.to_string(&txn))
                        .unwrap_or_default();
                    out.push(Member {
                        fingerprint: fp.to_string(),
                        role,
                        label,
                    });
                }
            }
        }
        out
    }

    /// The KB join policy (default invite).
    pub fn join_policy(&self) -> JoinPolicy {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_POLICY_KEY)
            .map(|v| v.to_string(&txn))
            .and_then(|s| JoinPolicy::parse(&s))
            .unwrap_or_default()
    }

    /// The KB's transport-exposure policy (ADR-018/025). **Absent ⇒ Hub** — a
    /// hub-shared KB is not mesh-reachable until explicitly p2p-shared.
    pub fn transport_policy(&self) -> TransportPolicy {
        self.transport_policy_raw().unwrap_or_default()
    }

    /// The transport policy as STORED — `None` when never explicitly set (vs an
    /// explicit `Hub`). `kb/share` widens from this so a never-shared KB shared
    /// over p2p becomes P2p-only, while a hub share + a p2p re-share become Both.
    pub fn transport_policy_raw(&self) -> Option<TransportPolicy> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_TRANSPORT_POLICY_KEY)
            .map(|v| v.to_string(&txn))
            .and_then(|s| TransportPolicy::parse(&s))
    }

    /// ADR-037 content-encryption mode for this KB; absent ⇒ [`Encryption::None`]
    /// (plaintext, the v0.14 default). The wiring reads this to decide whether content
    /// ops are encrypted under the per-KB content key.
    pub fn encryption(&self) -> Encryption {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_ENCRYPTION_KEY)
            .map(|v| v.to_string(&txn))
            .and_then(|s| Encryption::parse(&s))
            .unwrap_or_default()
    }

    /// Pending join requests (invite policy).
    pub fn pending(&self) -> Vec<PendingRequest> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        let mut out = Vec::new();
        if let Some(Out::YMap(p)) = root.get(&txn, COLL_PENDING_KEY) {
            for (fp, v) in p.iter(&txn) {
                if let Out::YMap(req) = v {
                    let label = req
                        .get(&txn, MEMBER_LABEL_KEY)
                        .map(|l| l.to_string(&txn))
                        .unwrap_or_default();
                    let requested_at = req
                        .get(&txn, PENDING_AT_KEY)
                        .map(|t| t.to_string(&txn))
                        .unwrap_or_default();
                    let pubkey = req
                        .get(&txn, PENDING_PUBKEY_KEY)
                        .map(|p| p.to_string(&txn))
                        .and_then(|h| hex::decode(h).ok())
                        .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok());
                    let wrap_pubkey = req
                        .get(&txn, PENDING_WRAP_PUBKEY_KEY)
                        .map(|p| p.to_string(&txn))
                        .and_then(|h| hex::decode(h).ok())
                        .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok());
                    out.push(PendingRequest {
                        fingerprint: fp.to_string(),
                        label,
                        requested_at,
                        pubkey,
                        wrap_pubkey,
                    });
                }
            }
        }
        out
    }

    /// Helper: get-or-create the `member_roles` YMap within an open txn.
    fn member_roles_map(root: &MapRef, txn: &mut yrs::TransactionMut) -> MapRef {
        match root.get(txn, COLL_MEMBER_ROLES_KEY) {
            Some(Out::YMap(m)) => m,
            _ => root.insert(txn, COLL_MEMBER_ROLES_KEY, MapPrelim::default()),
        }
    }

    /// The `retired` tombstone map (#72): fingerprint → last epoch of members that
    /// have been removed. A re-grant of a tombstoned principal issues a fresh
    /// epoch instead of resetting to the epoch-0 sentinel (which would reuse the
    /// pre-removal client_id and silently un-fence their old lineage).
    fn retired_map(root: &MapRef, txn: &mut yrs::TransactionMut) -> MapRef {
        match root.get(txn, COLL_RETIRED_KEY) {
            Some(Out::YMap(m)) => m,
            _ => root.insert(txn, COLL_RETIRED_KEY, MapPrelim::default()),
        }
    }

    /// Whether `principal` has a removal tombstone (was a write-capable member).
    fn is_retired(root: &MapRef, txn: &impl ReadTxn, principal: &str) -> bool {
        matches!(root.get(txn, COLL_RETIRED_KEY), Some(Out::YMap(m)) if m.get(txn, principal).is_some())
    }

    /// Bind the authoritative owner = `principal` (key fingerprint), display
    /// `label`. Idempotent; ensures schema=2 + default policy + the owner member
    /// entry. The daemon calls this on kb/share to bind the verified cert identity.
    pub fn set_owner(&mut self, principal: &str, label: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_OWNER_KEY, principal);
        root.insert(&mut txn, COLL_CREATOR_KEY, label);
        if root.get(&txn, COLL_SCHEMA_KEY).is_none() {
            root.insert(&mut txn, COLL_SCHEMA_KEY, SCHEMA_VERSION as i64);
        }
        if root.get(&txn, COLL_POLICY_KEY).is_none() {
            root.insert(&mut txn, COLL_POLICY_KEY, JoinPolicy::default().as_str());
        }
        if root.get(&txn, COLL_PENDING_KEY).is_none() {
            root.insert(&mut txn, COLL_PENDING_KEY, MapPrelim::default());
        }
        let m = Self::member_roles_map(&root, &mut txn);
        // Preserve the epoch on owner re-stamp (B-12 re-share — same owner, not a
        // role change); a brand-new owner seeds at epoch 0. The owner is never
        // removed via remove_principal (it is the authority), so never tombstoned.
        let prev = Self::entry_role_epoch(&m, &txn, principal);
        let epoch = Self::next_epoch(prev, Role::Owner, false);
        let entry = m.insert(&mut txn, principal, MapPrelim::default());
        entry.insert(&mut txn, MEMBER_ROLE_KEY, Role::Owner.as_str());
        entry.insert(&mut txn, MEMBER_LABEL_KEY, label);
        entry.insert(&mut txn, MEMBER_EPOCH_KEY, epoch.to_string());
        txn.encode_update_v1()
    }

    /// Read the current epoch of a member entry (within an open txn). 0 if absent.
    fn entry_epoch(entry: &MapRef, txn: &impl ReadTxn) -> u64 {
        entry
            .get(txn, MEMBER_EPOCH_KEY)
            .map(|v| v.to_string(txn))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
    }

    /// Read a member entry's current role (within an open txn).
    fn entry_role(entry: &MapRef, txn: &impl ReadTxn) -> Option<Role> {
        entry
            .get(txn, MEMBER_ROLE_KEY)
            .map(|v| v.to_string(txn))
            .and_then(|s| Role::parse(&s))
    }

    /// ADR-023 epoch transition. The authorization epoch advances **only when an
    /// existing member's role actually changes** — the B-19 cascade vector (e.g.
    /// viewer→editor). A *fresh* grant has no prior write-capable lineage to fence,
    /// so it stays at epoch 0; this is what lets owners and directly-added editors
    /// author under the base (epoch-0) client_id with no editor-side epoch sync. A
    /// role change rotates the client_id the member must author under, fencing their
    /// pre-change lineage at the daemon. (Monotonicity across remove/re-add is a
    /// documented hardening follow-up — a removed member's epoch is not persisted.)
    fn next_epoch(prev: Option<(Role, u64)>, new_role: Role, was_retired: bool) -> u64 {
        match prev {
            // Existing member, same role: no-op re-set, epoch unchanged.
            Some((prev_role, prev_epoch)) if prev_role == new_role => prev_epoch,
            // Existing member, role changed: advance to an unpredictable token
            // (#72 — was `prev_epoch + 1`, which a client could precompute).
            Some(_) => fresh_epoch_token(),
            // Re-grant of a previously-removed member: advance, never reset to 0
            // (#72 Part B — monotonicity across remove/re-add).
            None if was_retired => fresh_epoch_token(),
            // Genuinely-fresh grant to a never-seen principal: the epoch-0 sentinel
            // (no prior write-capable lineage to fence; owners/direct editors author
            // under the base client_id with no editor-side epoch sync).
            None => 0,
        }
    }

    /// Read a member entry's `(role, epoch)` for an epoch transition decision.
    fn entry_role_epoch(m: &MapRef, txn: &impl ReadTxn, principal: &str) -> Option<(Role, u64)> {
        match m.get(txn, principal) {
            Some(Out::YMap(e)) => {
                Self::entry_role(&e, txn).map(|r| (r, Self::entry_epoch(&e, txn)))
            }
            _ => None,
        }
    }

    /// Insert or update a member's role (keyed by principal; CRDT-safe LWW).
    /// ADR-023: any call here is a role (re)assignment, so it **bumps the member's
    /// authorization epoch** — rotating the KB client_id they must author under and
    /// fencing their pre-grant lineage at the daemon.
    pub fn upsert_member(&mut self, principal: &str, label: &str, role: Role) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        let was_retired = Self::is_retired(&root, &txn, principal);
        let m = Self::member_roles_map(&root, &mut txn);
        // Epoch advances on a role change of an existing member, or a re-grant of a
        // previously-removed one (#72); else it's a fresh grant at epoch 0 (ADR-023).
        let prev = Self::entry_role_epoch(&m, &txn, principal);
        let epoch = Self::next_epoch(prev, role, was_retired);
        let entry = m.insert(&mut txn, principal, MapPrelim::default());
        entry.insert(&mut txn, MEMBER_ROLE_KEY, role.as_str());
        entry.insert(&mut txn, MEMBER_LABEL_KEY, label);
        entry.insert(&mut txn, MEMBER_EPOCH_KEY, epoch.to_string());
        if was_retired {
            let r = Self::retired_map(&root, &mut txn);
            r.remove(&mut txn, principal); // member is active again — clear tombstone
        }
        txn.encode_update_v1()
    }

    /// Update only the role of an existing member (no-op if absent). Bumps the
    /// member's authorization epoch (ADR-023).
    pub fn set_role(&mut self, principal: &str, role: Role) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            if let Some(Out::YMap(entry)) = m.get(&txn, principal) {
                // Only an actual role change advances the epoch (ADR-023).
                let prev =
                    Self::entry_role(&entry, &txn).map(|r| (r, Self::entry_epoch(&entry, &txn)));
                // set_role only touches a present member, so there is no tombstone.
                let epoch = Self::next_epoch(prev, role, false);
                entry.insert(&mut txn, MEMBER_ROLE_KEY, role.as_str());
                entry.insert(&mut txn, MEMBER_EPOCH_KEY, epoch.to_string());
            }
        }
        txn.encode_update_v1()
    }

    /// The current authorization epoch of `principal` (ADR-023). 0 if not a member.
    pub fn epoch_of(&self, principal: &str) -> u64 {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            if let Some(Out::YMap(entry)) = m.get(&txn, principal) {
                return Self::entry_epoch(&entry, &txn);
            }
        }
        0
    }

    /// Remove a member by principal.
    pub fn remove_principal(&mut self, principal: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        // #72: tombstone the removed member's epoch so a later re-grant issues a
        // fresh epoch (never reuses the pre-removal client_id and silently
        // un-fences the removed member's old lineage).
        let prev_epoch = match root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            Some(Out::YMap(m)) => {
                let e = Self::entry_role_epoch(&m, &txn, principal).map(|(_, ep)| ep);
                m.remove(&mut txn, principal);
                e
            }
            _ => None,
        };
        if let Some(e) = prev_epoch {
            let r = Self::retired_map(&root, &mut txn);
            r.insert(&mut txn, principal, e.to_string());
        }
        txn.encode_update_v1()
    }

    /// Set the KB join policy.
    pub fn set_join_policy(&mut self, policy: JoinPolicy) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_POLICY_KEY, policy.as_str());
        txn.encode_update_v1()
    }

    /// Set the KB's transport-exposure policy (owner-only at the gate).
    pub fn set_transport_policy(&mut self, policy: TransportPolicy) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_TRANSPORT_POLICY_KEY, policy.as_str());
        txn.encode_update_v1()
    }

    /// Set this KB's ADR-037 content-encryption mode (owner op). Returns the encoded
    /// yrs update for persist+broadcast, like the other collection setters.
    pub fn set_encryption(&mut self, mode: Encryption) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_ENCRYPTION_KEY, mode.as_str());
        txn.encode_update_v1()
    }

    /// Record a pending join request (idempotent re-request). `pubkey` (ADR-038) is the
    /// joiner's Ed25519 key, captured by the daemon from the authenticated session so the
    /// owner can wrap the content key to them on approval; `None` preserves the v1 record.
    pub fn add_pending(
        &mut self,
        principal: &str,
        label: &str,
        requested_at: &str,
        pubkey: Option<&[u8; 32]>,
        wrap_pubkey: Option<&[u8; 32]>,
    ) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        let p = match root.get(&txn, COLL_PENDING_KEY) {
            Some(Out::YMap(p)) => p,
            _ => root.insert(&mut txn, COLL_PENDING_KEY, MapPrelim::default()),
        };
        let req = p.insert(&mut txn, principal, MapPrelim::default());
        req.insert(&mut txn, MEMBER_LABEL_KEY, label);
        req.insert(&mut txn, PENDING_AT_KEY, requested_at);
        if let Some(pk) = pubkey {
            req.insert(&mut txn, PENDING_PUBKEY_KEY, hex::encode(pk));
        }
        // ADR-041 (#158 I1): the joiner's published X25519 wrap key — what the owner wraps
        // the content key to. Sent by the joiner (the daemon can't derive it).
        if let Some(wk) = wrap_pubkey {
            req.insert(&mut txn, PENDING_WRAP_PUBKEY_KEY, hex::encode(wk));
        }
        txn.encode_update_v1()
    }

    /// Remove a pending request.
    pub fn remove_pending(&mut self, principal: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(p)) = root.get(&txn, COLL_PENDING_KEY) {
            p.remove(&mut txn, principal);
        }
        txn.encode_update_v1()
    }

    /// Approve a pending principal as `role` — removes pending + adds the member
    /// in a SINGLE transaction (atomic, no transient half-state on peers).
    pub fn approve(&mut self, principal: &str, role: Role) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        let mut label = String::new();
        if let Some(Out::YMap(p)) = root.get(&txn, COLL_PENDING_KEY) {
            if let Some(Out::YMap(req)) = p.get(&txn, principal) {
                label = req
                    .get(&txn, MEMBER_LABEL_KEY)
                    .map(|l| l.to_string(&txn))
                    .unwrap_or_default();
            }
            p.remove(&mut txn, principal);
        }
        let was_retired = Self::is_retired(&root, &txn, principal);
        let m = Self::member_roles_map(&root, &mut txn);
        // Approving a pending principal is a fresh grant into member_roles (a denied
        // pending peer has no write-capable lineage); epoch 0 unless this re-grants
        // an existing member at a new role, or re-admits a previously-removed one
        // (#72 — the latter takes a fresh epoch, not the 0 sentinel).
        let prev = Self::entry_role_epoch(&m, &txn, principal);
        let epoch = Self::next_epoch(prev, role, was_retired);
        let entry = m.insert(&mut txn, principal, MapPrelim::default());
        entry.insert(&mut txn, MEMBER_ROLE_KEY, role.as_str());
        entry.insert(&mut txn, MEMBER_LABEL_KEY, label.as_str());
        entry.insert(&mut txn, MEMBER_EPOCH_KEY, epoch.to_string());
        if was_retired {
            let r = Self::retired_map(&root, &mut txn);
            r.remove(&mut txn, principal);
        }
        txn.encode_update_v1()
    }

    // --- ADR-026 signed membership op-log (the v3 source of truth) ---

    /// Get-or-create the membership op-log YMap within an open txn.
    fn oplog_map(root: &MapRef, txn: &mut yrs::TransactionMut) -> MapRef {
        match root.get(txn, COLL_OPLOG_KEY) {
            Some(Out::YMap(m)) => m,
            _ => root.insert(txn, COLL_OPLOG_KEY, MapPrelim::default()),
        }
    }

    /// Decode one op record (a YMap value) into a [`SignedMembershipOp`]. Returns
    /// `None` if a required field is missing or malformed — a corrupt/partial
    /// record simply doesn't contribute to derivation (fail-closed, never panics).
    fn decode_op_record(rec: &MapRef, txn: &impl ReadTxn) -> Option<SignedMembershipOp> {
        let get = |k: &str| rec.get(txn, k).map(|v| v.to_string(txn));
        let role_s = get(OP_ROLE_KEY).unwrap_or_default();
        let role = if role_s.is_empty() {
            None
        } else {
            Some(Role::parse(&role_s)?)
        };
        let expires_s = get(OP_EXPIRES_KEY).unwrap_or_default();
        let expires_at = if expires_s.is_empty() {
            None
        } else {
            Some(expires_s.parse::<u64>().ok()?)
        };
        let sig = hex::decode(get(OP_SIG_KEY)?).ok()?;
        let pubkey: [u8; 32] = hex::decode(get(OP_PUBKEY_KEY)?).ok()?.try_into().ok()?;
        Some(SignedMembershipOp {
            op: MembershipOp {
                kb_id: get(OP_KBID_KEY)?,
                action: MembershipAction::parse(&get(OP_ACTION_KEY)?)?,
                subject: get(OP_SUBJECT_KEY)?,
                role,
                can_invite: get(OP_CAN_INVITE_KEY).as_deref() == Some("1"),
                author: get(OP_AUTHOR_KEY)?,
                issued_at: get(OP_ISSUED_KEY)?.parse::<u64>().ok()?,
                expires_at,
                epoch: get(OP_EPOCH_KEY)
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0),
                prev_hash: get(OP_PREV_KEY).unwrap_or_default(),
                // ADR-037: present only on encrypted-KB admits. A malformed hex value
                // decodes to None — the op then derives as v1, fails its v2 signature,
                // and is dropped by `verify_signed`, so it can't smuggle a bad key.
                wrapped_key: get(OP_WRAPPED_KEY)
                    .filter(|s| !s.is_empty())
                    .and_then(|s| hex::decode(s).ok()),
                // ADR-040: present only on Rebind ops. A malformed/missing hex value decodes
                // to None — the op then derives as the wrong version, fails its v3 signature,
                // and is dropped by `verify_signed`, so it can't smuggle a bad successor key.
                new_pubkey: get(OP_NEW_PUBKEY_KEY)
                    .filter(|s| !s.is_empty())
                    .and_then(|s| hex::decode(s).ok())
                    .and_then(|b| b.try_into().ok()),
                new_wrap_pubkey: get(OP_NEW_WRAP_PUBKEY_KEY)
                    .filter(|s| !s.is_empty())
                    .and_then(|s| hex::decode(s).ok())
                    .and_then(|b| b.try_into().ok()),
                // ADR-040 §Recovery-key: present only on RegisterRecoveryKey ops (v4).
                recovery_pubkey: get(OP_RECOVERY_PUBKEY_KEY)
                    .filter(|s| !s.is_empty())
                    .and_then(|s| hex::decode(s).ok())
                    .and_then(|b| b.try_into().ok()),
            },
            sig,
            author_pubkey: pubkey,
        })
    }

    /// All signed membership ops in the log, in arbitrary order. Validity
    /// derivation (`derive_valid_members`) orders them by the `prev_hash` causal
    /// DAG and applies the resolver; this reader does no validation beyond decode.
    pub fn oplog_ops(&self) -> Vec<SignedMembershipOp> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        let mut out = Vec::new();
        if let Some(Out::YMap(log)) = root.get(&txn, COLL_OPLOG_KEY) {
            for (_key, v) in log.iter(&txn) {
                if let Out::YMap(rec) = v {
                    if let Some(op) = Self::decode_op_record(&rec, &txn) {
                        out.push(op);
                    }
                }
            }
        }
        out
    }

    /// Number of records in the op-log (decoded + malformed alike).
    pub fn oplog_len(&self) -> usize {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_OPLOG_KEY) {
            Some(Out::YMap(log)) => log.len(&txn) as usize,
            _ => 0,
        }
    }

    /// The current frontier head of the op-log DAG — the `chain_hash` to use as the
    /// next op's `prev_hash`. A tip is an op whose hash is no other op's `prev_hash`;
    /// with multiple concurrent tips the highest hash is chosen (deterministic, so
    /// every honest builder agrees). `None` ⇒ empty log (the next op is genesis).
    pub fn oplog_head(&self) -> Option<String> {
        let ops = self.oplog_ops();
        let referenced: Vec<String> = ops
            .iter()
            .map(|o| o.op.prev_hash.clone())
            .filter(|p| !p.is_empty())
            .collect();
        ops.iter()
            .map(|o| o.chain_hash())
            .filter(|h| !referenced.iter().any(|r| r == h))
            .max()
    }

    /// Build an unsigned membership op linked to the current op-log head (pure — no
    /// key, no mutation). The daemon signs the returned op with the authoring
    /// identity, then calls [`append_signed_op`](Self::append_signed_op). `prev_hash`
    /// is the author's view-head ([`oplog_head`](Self::oplog_head)) so the op extends
    /// the causal DAG; the genesis op (empty log) gets `prev_hash = ""`.
    #[allow(clippy::too_many_arguments)]
    pub fn build_membership_op(
        &self,
        kb_id: &str,
        action: MembershipAction,
        subject: &str,
        role: Option<Role>,
        can_invite: bool,
        author: &str,
        issued_at: u64,
        expires_at: Option<u64>,
        epoch: u64,
    ) -> MembershipOp {
        MembershipOp {
            kb_id: kb_id.to_string(),
            action,
            subject: subject.to_string(),
            role,
            can_invite,
            author: author.to_string(),
            issued_at,
            expires_at,
            epoch,
            prev_hash: self.oplog_head().unwrap_or_default(),
            // ADR-037: the caller sets this on an encrypted-KB admit (then signs);
            // the daemon's existing membership flows leave it None (v1, unchanged).
            wrapped_key: None,
            // ADR-040: the caller sets these on a Rebind (then signs); all other ops
            // leave them None (v1/v2, unchanged).
            new_pubkey: None,
            new_wrap_pubkey: None,
            // ADR-040 §Recovery: the caller sets this on a RegisterRecoveryKey (then signs).
            recovery_pubkey: None,
        }
    }

    /// Append a signed op to the log, keyed by its `chain_hash` (so concurrent
    /// appends of distinct ops converge as a set, and a re-append is idempotent).
    /// Stores the op fields + signature + author pubkey so the record is
    /// independently verifiable by any peer. Returns the encoded yrs update.
    ///
    /// This does **not** validate the op — appending and *deriving validity* are
    /// separate (a relay may carry an invalid op; `derive_valid_members` is what
    /// refuses to count it). The daemon gates the author's capability *before*
    /// appending (Phase 2b-6).
    pub fn append_signed_op(
        &mut self,
        op: &MembershipOp,
        sig: &[u8],
        author_pubkey: &[u8; 32],
    ) -> Vec<u8> {
        let key = op.chain_hash(sig);
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        let log = Self::oplog_map(&root, &mut txn);
        let rec = log.insert(&mut txn, key.as_str(), MapPrelim::default());
        rec.insert(&mut txn, OP_KBID_KEY, op.kb_id.as_str());
        rec.insert(&mut txn, OP_ACTION_KEY, op.action.as_str());
        rec.insert(&mut txn, OP_SUBJECT_KEY, op.subject.as_str());
        rec.insert(
            &mut txn,
            OP_ROLE_KEY,
            op.role.map(|r| r.as_str()).unwrap_or(""),
        );
        rec.insert(
            &mut txn,
            OP_CAN_INVITE_KEY,
            if op.can_invite { "1" } else { "0" },
        );
        rec.insert(&mut txn, OP_AUTHOR_KEY, op.author.as_str());
        rec.insert(&mut txn, OP_ISSUED_KEY, op.issued_at.to_string());
        rec.insert(
            &mut txn,
            OP_EXPIRES_KEY,
            op.expires_at.map(|e| e.to_string()).unwrap_or_default(),
        );
        rec.insert(&mut txn, OP_EPOCH_KEY, op.epoch.to_string());
        rec.insert(&mut txn, OP_PREV_KEY, op.prev_hash.as_str());
        rec.insert(&mut txn, OP_SIG_KEY, hex::encode(sig));
        rec.insert(&mut txn, OP_PUBKEY_KEY, hex::encode(author_pubkey));
        // ADR-037: only written for an encrypted-KB admit (absent ⇒ v1, unchanged).
        if let Some(wk) = &op.wrapped_key {
            rec.insert(&mut txn, OP_WRAPPED_KEY, hex::encode(wk));
        }
        // ADR-040: only written for a Rebind (absent ⇒ unchanged v1/v2).
        if let Some(pk) = &op.new_pubkey {
            rec.insert(&mut txn, OP_NEW_PUBKEY_KEY, hex::encode(pk));
        }
        if let Some(wpk) = &op.new_wrap_pubkey {
            rec.insert(&mut txn, OP_NEW_WRAP_PUBKEY_KEY, hex::encode(wpk));
        }
        // ADR-040 §Recovery: only written for a RegisterRecoveryKey (absent ⇒ unchanged).
        if let Some(rpk) = &op.recovery_pubkey {
            rec.insert(&mut txn, OP_RECOVERY_PUBKEY_KEY, hex::encode(rpk));
        }
        txn.encode_update_v1()
    }

    /// ADVERSARIAL-TEST ONLY: remove an op-log record by its `chain_hash` and return the
    /// resulting delta. The membership op-log is APPEND-ONLY in production (no code path
    /// deletes) — this exists so tests can construct the deletion attack the daemon's
    /// grow-only self-service gate must reject (a member dropping a co-member's `Admit`, the
    /// owner's `SetEncryption`, or the genesis). Safe to expose: any delta it produces is
    /// rejected by that gate, so it cannot be used to actually mutate a shared KB.
    #[doc(hidden)]
    pub fn remove_oplog_op_for_test(&mut self, chain_hash: &str) -> Vec<u8> {
        let sv = self.state_vector();
        {
            let root = self.doc.get_or_insert_map(COLLECTION_MAP);
            let mut txn = self.doc.transact_mut();
            let log = Self::oplog_map(&root, &mut txn);
            log.remove(&mut txn, chain_hash);
        }
        let sv_d = yrs::StateVector::decode_v1(&sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&sv_d)
    }

    /// ADR-037/039: enable E2E encryption on an owned KB. Authors, in ONE combined
    /// collection delta (a state-vector diff), all of:
    /// - the **genesis owner self-admit** (the trust anchor `derive_*` require), carrying
    ///   the owner's **self-wrapped** content key so the owner can recover it (skipped if
    ///   the op-log already has a genesis — idempotent);
    /// - the signed **`SetEncryption("e2e")`** op — the monotonic, anti-downgrade mode
    ///   source read by [`crate::membership::derive_encryption`] (ADR-039 F2);
    /// - the unsigned `Encryption::E2e` flag, for backward-compat display only (the
    ///   authoritative mode is the signed op).
    ///
    /// Returns the delta to ship via `kb/collection_op`; the daemon stores it key-blind
    /// (ADR-038). `owner_fp` MUST be `fingerprint_of(owner_pubkey)` (the signature binds
    /// author↔key↔fingerprint).
    pub fn author_e2e_genesis(
        &mut self,
        kb_id: &str,
        owner_fp: &str,
        owner_secret: &[u8; 32],
        owner_pubkey: &[u8; 32],
        self_wrapped_key: Vec<u8>,
        now: u64,
    ) -> Vec<u8> {
        let sv = self.state_vector();
        // Genesis self-admit (anchor) carrying the owner's wrapped key — only if absent.
        if self.oplog_head().is_none() {
            let mut g = self.build_membership_op(
                kb_id,
                MembershipAction::Admit,
                owner_fp,
                Some(Role::Owner),
                true,
                owner_fp,
                now,
                None,
                0,
            );
            g.wrapped_key = Some(self_wrapped_key);
            let sig = g.sign(owner_secret);
            self.append_signed_op(&g, &sig, owner_pubkey);
        }
        // Signed SetEncryption(e2e); `build_membership_op` chains it onto the current head.
        let se = self.build_membership_op(
            kb_id,
            MembershipAction::SetEncryption,
            "e2e",
            None,
            false,
            owner_fp,
            now,
            None,
            0,
        );
        let sig = se.sign(owner_secret);
        self.append_signed_op(&se, &sig, owner_pubkey);
        // Backward-compat unsigned flag (authoritative mode = derive_encryption).
        self.set_encryption(Encryption::E2e);
        // ONE combined delta capturing the genesis + SetEncryption + flag.
        let sv_d = yrs::StateVector::decode_v1(&sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&sv_d)
    }

    /// ADR-038: author a signed `Admit` of `subject` at `role`, carrying the content key
    /// `wrapped_key` (ADR-037, wrapped to `subject_pubkey`), AND mirror the member into
    /// `member_roles` (role + epoch + the pubkey) — all in ONE combined collection delta.
    /// The op's epoch == the `member_roles` epoch, so the ADR-023 fence stays consistent
    /// (the dual-write). `subject_pubkey` is also stored for later re-wrap on rotation.
    /// Returns the delta to ship via `kb/collection_op`.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn author_member_admit(
        &mut self,
        kb_id: &str,
        subject_fp: &str,
        subject_pubkey: &[u8; 32],
        subject_wrap_pubkey: &[u8; 32],
        role: Role,
        label: &str,
        wrapped_key: Vec<u8>,
        owner_fp: &str,
        owner_secret: &[u8; 32],
        owner_pubkey: &[u8; 32],
        now: u64,
    ) -> Vec<u8> {
        let sv = self.state_vector();
        // Mirror into member_roles (sets role + advances epoch) + store the pubkeys.
        self.upsert_member(subject_fp, label, role);
        self.store_member_pubkey(subject_fp, subject_pubkey);
        self.store_member_wrap_pubkey(subject_fp, subject_wrap_pubkey); // ADR-041 I1

        // Author the signed Admit at the SAME epoch member_roles just assigned.
        let epoch = self.epoch_of(subject_fp);
        let mut op = self.build_membership_op(
            kb_id,
            MembershipAction::Admit,
            subject_fp,
            Some(role),
            false,
            owner_fp,
            now,
            None,
            epoch,
        );
        op.wrapped_key = Some(wrapped_key);
        let sig = op.sign(owner_secret);
        self.append_signed_op(&op, &sig, owner_pubkey);
        let sv_d = yrs::StateVector::decode_v1(&sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&sv_d)
    }

    /// ADR-037 §D3: **rotate the content key on member removal.** Authors, in ONE combined
    /// collection delta: (1) a signed `Remove` of `removed_fp` (and mirrors the member_roles
    /// removal, which #72-tombstones their epoch), then (2) one owner-authored *wrap-only*
    /// `Admit` per REMAINING member carrying the NEW key wrapped to them. Each re-key op
    /// re-asserts the member's CURRENT derived role/can_invite/epoch verbatim — a re-admit
    /// overwrites the derived entry ("later re-admit wins"), so preserving them avoids a
    /// silent membership downgrade; and the epoch is NOT bumped (re-keying must not force the
    /// remaining members to rebase — the removed member is dropped from derived membership, so
    /// their stale lineage is refused regardless of epoch).
    ///
    /// `rewraps` is `(remaining_member_fp, new_wrapped_key)` for every member to KEEP — the
    /// caller (which holds the secret + the members' pubkeys) wraps the fresh key once per
    /// member; the owner re-keys itself by appearing in this list. The removed member receives
    /// no new wrapped op, so `find_wrapped_content_key` returns only their OLD key — they
    /// cannot open post-rotation ciphertext (the §D3 security property). Returns the delta to
    /// ship via the key-blind `kb/collection_op`.
    #[allow(clippy::too_many_arguments)]
    pub fn author_rotate_on_remove(
        &mut self,
        kb_id: &str,
        removed_fp: &str,
        rewraps: &[(String, Vec<u8>)],
        owner_fp: &str,
        owner_secret: &[u8; 32],
        owner_pubkey: &[u8; 32],
        now: u64,
    ) -> Vec<u8> {
        let sv = self.state_vector();
        // Snapshot the authoritative attributes BEFORE the Remove so each re-key Admit can
        // re-assert them verbatim (the op-log is the source of truth; `owner_pubkey` anchors).
        let ops = self.oplog_ops();
        let governance = crate::membership::derive_governance(&ops, owner_pubkey);
        let members = crate::membership::derive_valid_members_governed(
            &ops,
            owner_pubkey,
            now,
            governance,
            &crate::membership::MembershipView::default(),
        );
        // (1) Signed Remove of the departed member + the member_roles mirror.
        let remove_op = self.build_membership_op(
            kb_id,
            MembershipAction::Remove,
            removed_fp,
            None,
            false,
            owner_fp,
            now,
            None,
            0,
        );
        let sig = remove_op.sign(owner_secret);
        self.append_signed_op(&remove_op, &sig, owner_pubkey);
        self.remove_principal(removed_fp);
        // (2) One wrap-only re-key Admit per remaining member, current attributes preserved.
        for (member_fp, wrapped) in rewraps {
            if member_fp == removed_fp {
                continue; // defensive: never re-key the member we just removed
            }
            let (role, can_invite, epoch) = members
                .get(member_fp)
                .map(|m| (m.role, m.can_invite, m.epoch))
                .unwrap_or((Role::Editor, false, self.epoch_of(member_fp)));
            let mut op = self.build_membership_op(
                kb_id,
                MembershipAction::Admit,
                member_fp,
                Some(role),
                can_invite,
                owner_fp,
                now,
                None,
                epoch,
            );
            op.wrapped_key = Some(wrapped.clone());
            let sig = op.sign(owner_secret);
            self.append_signed_op(&op, &sig, owner_pubkey);
        }
        let sv_d = yrs::StateVector::decode_v1(&sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&sv_d)
    }

    /// ADR-040 §1-2: author an identity-rotation `Rebind` into this KB's signed op-log.
    /// The OLD key (`old_secret`/`old_pubkey`, fingerprint `old_fp`) cross-signs the
    /// successor `new_fp` (which MUST equal `fingerprint_of(new_pubkey)`), publishing the
    /// successor's Ed25519 (`new_pubkey`) + X25519 wrap (`new_wrap_pubkey`, ADR-041/I1)
    /// keys so peers learn the new node-id and the owner can re-wrap the content key.
    /// Honoring + retirement (the successor inherits the predecessor's exact role/epoch;
    /// the old key's later ops stop being honored) are derived per-peer by
    /// `derive_valid_members`. Returns the delta for the key-blind `kb/collection_op`.
    #[allow(clippy::too_many_arguments)]
    pub fn author_rebind(
        &mut self,
        kb_id: &str,
        old_fp: &str,
        new_fp: &str,
        new_pubkey: &[u8; 32],
        new_wrap_pubkey: &[u8; 32],
        old_secret: &[u8; 32],
        old_pubkey: &[u8; 32],
        now: u64,
    ) -> Vec<u8> {
        let sv = self.state_vector();
        // subject = successor, author = predecessor (the OLD key signs). Role/epoch are
        // inherited in derivation, so they are unset here (0/None).
        let mut op = self.build_membership_op(
            kb_id,
            MembershipAction::Rebind,
            new_fp,
            None,
            false,
            old_fp,
            now,
            None,
            0,
        );
        op.new_pubkey = Some(*new_pubkey);
        op.new_wrap_pubkey = Some(*new_wrap_pubkey);
        let sig = op.sign(old_secret);
        self.append_signed_op(&op, &sig, old_pubkey);
        let sv_d = yrs::StateVector::decode_v1(&sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&sv_d)
    }

    /// ADR-040 §Recovery-key — register `principal_fp`'s offline **recovery key** (its public
    /// `recovery_pubkey`), SIGNED BY THE PRIMARY (`primary_secret`/`primary_pubkey`) while it
    /// is uncompromised. Self-targeted (`subject == author == principal_fp`). Peers store it in
    /// the recovery registry so a later `author_recovery_rebind` signed by the matching
    /// recovery secret is honored. Latest registration wins (revokes a leaked recovery key).
    /// Returns the delta for the key-blind `kb/collection_op`.
    #[allow(clippy::too_many_arguments)]
    pub fn author_register_recovery_key(
        &mut self,
        kb_id: &str,
        principal_fp: &str,
        recovery_pubkey: &[u8; 32],
        primary_secret: &[u8; 32],
        primary_pubkey: &[u8; 32],
        now: u64,
    ) -> Vec<u8> {
        let sv = self.state_vector();
        let mut op = self.build_membership_op(
            kb_id,
            MembershipAction::RegisterRecoveryKey,
            principal_fp, // subject == author (self-registration)
            None,
            false,
            principal_fp,
            now,
            None,
            0,
        );
        op.recovery_pubkey = Some(*recovery_pubkey);
        let sig = op.sign(primary_secret);
        self.append_signed_op(&op, &sig, primary_pubkey);
        let sv_d = yrs::StateVector::decode_v1(&sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&sv_d)
    }

    /// ADR-040 §Recovery-key — rotate `old_fp` to a fresh successor using the pre-registered
    /// **recovery key** (compromise/loss recovery, when the primary can no longer sign). The
    /// op is a normal `Rebind` (author = `old_fp` = the recovered principal; subject =
    /// `new_fp`) but the RECORD is signed by the recovery secret and stamped with
    /// `recovery_pubkey`. `verify_signed` is false for it (the signer ≠ `old_fp`), so peers
    /// honor it only via the recovery registry — i.e. iff `recovery_pubkey` is the registered
    /// recovery key for `old_fp` (`is_recovery_signed_rebind`). Returns the `kb/collection_op`
    /// delta. The successor inherits `old_fp`'s exact role/epoch (no elevation).
    #[allow(clippy::too_many_arguments)]
    pub fn author_recovery_rebind(
        &mut self,
        kb_id: &str,
        old_fp: &str,
        new_fp: &str,
        new_pubkey: &[u8; 32],
        new_wrap_pubkey: &[u8; 32],
        recovery_secret: &[u8; 32],
        recovery_pubkey: &[u8; 32],
        now: u64,
    ) -> Vec<u8> {
        let sv = self.state_vector();
        let mut op = self.build_membership_op(
            kb_id,
            MembershipAction::Rebind,
            new_fp,
            None,
            false,
            old_fp, // author = the principal being recovered (NOT the recovery key)
            now,
            None,
            0,
        );
        op.new_pubkey = Some(*new_pubkey);
        op.new_wrap_pubkey = Some(*new_wrap_pubkey);
        // Signed by the RECOVERY key; the record's author_pubkey is the recovery pubkey.
        let sig = op.sign(recovery_secret);
        self.append_signed_op(&op, &sig, recovery_pubkey);
        let sv_d = yrs::StateVector::decode_v1(&sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&sv_d)
    }

    /// ADR-040 §3: the owner re-wraps the CURRENT content key to a rotated member's
    /// successor `new_fp`. Authors one owner-signed wrap-only `Admit` carrying `wrapped`
    /// (the content key the caller sealed to the successor's published X25519 wrap key) and
    /// re-asserting the successor's INHERITED role/can_invite/epoch verbatim (read from the
    /// post-rebind derived membership) — so `derive_content_key` resolves for the new key
    /// WITHOUT bumping the epoch (no forced rebase) or changing membership. Returns the
    /// delta for the key-blind `kb/collection_op`. The caller holds the secret + content
    /// key and does the sealing; this only authors the signed delivery op.
    ///
    /// `anchor_pubkey` is the KB's **genesis** owner pubkey — the trust anchor, which never
    /// changes across rotations — used to DERIVE membership. `signer_*` is the **current**
    /// owner authoring the re-wrap, which may be a rotated successor distinct from the anchor:
    /// on OWNER self-rotation the old owner key is retired the instant its Rebind lands, so
    /// the re-wrap MUST be signed by the NEW owner key while derivation still anchors on the
    /// original genesis. (For a member rotation re-wrapped by a stable owner, pass the same
    /// key as both — `anchor == signer`.) The signer is honored because it is in the genesis
    /// owner's rebind chain (`owner_principal_chain`).
    #[allow(clippy::too_many_arguments)]
    pub fn author_rebind_rewrap(
        &mut self,
        kb_id: &str,
        new_fp: &str,
        new_pubkey: &[u8; 32],
        wrapped: Vec<u8>,
        anchor_pubkey: &[u8; 32],
        signer_fp: &str,
        signer_secret: &[u8; 32],
        signer_pubkey: &[u8; 32],
        now: u64,
    ) -> Vec<u8> {
        let sv = self.state_vector();
        // Read the successor's inherited attributes from the post-rebind derived membership
        // (the rebind already aliased old→new with old's role/epoch). Anchor on the GENESIS
        // owner pubkey — the successor is not the anchor. Fall back defensively.
        let ops = self.oplog_ops();
        let governance = crate::membership::derive_governance(&ops, anchor_pubkey);
        let members = crate::membership::derive_valid_members_governed(
            &ops,
            anchor_pubkey,
            now,
            governance,
            &crate::membership::MembershipView::default(),
        );
        let (role, can_invite, epoch) = members
            .get(new_fp)
            .map(|m| (m.role, m.can_invite, m.epoch))
            .unwrap_or((Role::Editor, false, self.epoch_of(new_fp)));
        let _ = new_pubkey; // successor pubkey already published in the Rebind op
        let mut op = self.build_membership_op(
            kb_id,
            MembershipAction::Admit,
            new_fp,
            Some(role),
            can_invite,
            signer_fp,
            now,
            None,
            epoch,
        );
        op.wrapped_key = Some(wrapped);
        let sig = op.sign(signer_secret);
        self.append_signed_op(&op, &sig, signer_pubkey);
        let sv_d = yrs::StateVector::decode_v1(&sv).unwrap_or_default();
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&sv_d)
    }

    /// Store a member's Ed25519 pubkey in their `member_roles` entry (for re-wrap on
    /// rotation). No-op if the member entry doesn't exist yet.
    fn store_member_pubkey(&mut self, principal: &str, pubkey: &[u8; 32]) {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            if let Some(Out::YMap(entry)) = m.get(&txn, principal) {
                entry.insert(&mut txn, MEMBER_PUBKEY_KEY, hex::encode(pubkey));
            }
        }
    }

    /// A member's stored Ed25519 pubkey (ADR-038), if recorded — for re-wrap on rotation.
    pub fn member_pubkey(&self, principal: &str) -> Option<[u8; 32]> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            if let Some(Out::YMap(entry)) = m.get(&txn, principal) {
                return entry
                    .get(&txn, MEMBER_PUBKEY_KEY)
                    .map(|p| p.to_string(&txn))
                    .and_then(|h| hex::decode(h).ok())
                    .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok());
            }
        }
        None
    }

    /// ADR-041 (#158 I1): record a member's PUBLISHED X25519 wrap key (within the open
    /// admit/genesis txn — same delta as the role), so rotation can re-wrap the content
    /// key to it. No-op if the member entry doesn't exist yet.
    fn store_member_wrap_pubkey(&mut self, principal: &str, wrap_pubkey: &[u8; 32]) {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            if let Some(Out::YMap(entry)) = m.get(&txn, principal) {
                entry.insert(&mut txn, MEMBER_WRAP_PUBKEY_KEY, hex::encode(wrap_pubkey));
            }
        }
    }

    /// A member's stored X25519 wrap key (ADR-041), if recorded — the key the owner wraps
    /// the content key to on admit/rotation.
    pub fn member_wrap_pubkey(&self, principal: &str) -> Option<[u8; 32]> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            if let Some(Out::YMap(entry)) = m.get(&txn, principal) {
                return entry
                    .get(&txn, MEMBER_WRAP_PUBKEY_KEY)
                    .map(|p| p.to_string(&txn))
                    .and_then(|h| hex::decode(h).ok())
                    .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok());
            }
        }
        None
    }

    /// Legacy v1 members (the read-only `members` YArray of labels), for migration.
    pub fn legacy_members(&self) -> Vec<String> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_MEMBERS_KEY) {
            Some(Out::YArray(arr)) => arr.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        }
    }

    /// Migrate a legacy v1 collection (label `creator` + `members` YArray) to the
    /// v2 identity-anchored schema. Idempotent: returns `None` if already v2.
    ///
    /// `resolver(label) -> Some((fingerprint, label))` maps a legacy label to its
    /// key principal (e.g. via the daemon's authorized_keys). A label that doesn't
    /// resolve becomes a transitional `legacy:<label>` principal — preserved for
    /// audit, but a real key peer won't match it, so the owner should re-add it by
    /// fingerprint (or simply re-share, which `set_owner` re-binds). The legacy
    /// `members` YArray is left intact (read-only); v2 data lives under new keys.
    pub fn migrate_if_legacy<F>(&mut self, resolver: F) -> Option<Vec<u8>>
    where
        F: Fn(&str) -> Option<(String, String)>,
    {
        if self.schema_version() >= 2 {
            return None;
        }
        let creator_label = self.creator();
        let legacy = self.legacy_members();
        // Resolve a label → (principal, label); fall back to legacy:<label>.
        let resolve = |label: &str| -> (String, String) {
            resolver(label).unwrap_or_else(|| (format!("legacy:{label}"), label.to_string()))
        };
        let (owner_principal, owner_label) = resolve(&creator_label);
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_SCHEMA_KEY, SCHEMA_VERSION as i64);
        root.insert(&mut txn, COLL_OWNER_KEY, owner_principal.as_str());
        if root.get(&txn, COLL_POLICY_KEY).is_none() {
            root.insert(&mut txn, COLL_POLICY_KEY, JoinPolicy::default().as_str());
        }
        if root.get(&txn, COLL_PENDING_KEY).is_none() {
            root.insert(&mut txn, COLL_PENDING_KEY, MapPrelim::default());
        }
        let m = Self::member_roles_map(&root, &mut txn);
        // Owner entry first.
        {
            let e = m.insert(&mut txn, owner_principal.as_str(), MapPrelim::default());
            e.insert(&mut txn, MEMBER_ROLE_KEY, Role::Owner.as_str());
            e.insert(&mut txn, MEMBER_LABEL_KEY, owner_label.as_str());
        }
        for label in legacy {
            let (principal, disp) = resolve(&label);
            if principal == owner_principal {
                continue; // already the owner
            }
            let e = m.insert(&mut txn, principal.as_str(), MapPrelim::default());
            e.insert(&mut txn, MEMBER_ROLE_KEY, Role::Editor.as_str());
            e.insert(&mut txn, MEMBER_LABEL_KEY, disp.as_str());
        }
        Some(txn.encode_update_v1())
    }

    /// Access the underlying Doc.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- KbNodeDoc tests ---

    #[test]
    fn new_node_schema() {
        let node = KbNodeDoc::new(
            "concept:test",
            "Test Node",
            "Some body text",
            &["tag1".to_string(), "tag2".to_string()],
        );
        assert_eq!(node.id(), "concept:test");
        assert_eq!(node.title(), "Test Node");
        assert_eq!(node.body(), "Some body text");
        assert_eq!(node.tags(), vec!["tag1", "tag2"]);
        assert!(node.links().is_empty());
    }

    #[test]
    fn set_tags_replaces_and_syncs() {
        // B-18: set_tags produces a real CRDT delta that converges a peer's tags.
        let mut owner = KbNodeDoc::new("n1", "T", "b", &["a".to_string(), "b".to_string()]);
        // Peer shares the lineage (loaded from the owner's encoded state).
        let mut peer = KbNodeDoc::from_bytes(&owner.encode()).unwrap();
        let sv = peer.state_vector();
        assert_eq!(peer.tags(), vec!["a", "b"]);

        // Owner replaces the tag set → diff → peer applies → converges.
        owner.set_tags(&["a".to_string(), "c".to_string()]);
        assert_eq!(owner.tags(), vec!["a", "c"]);
        let diff = owner.encode_diff(&sv).unwrap();
        peer.apply_update(&diff).unwrap();
        assert_eq!(
            peer.tags(),
            vec!["a", "c"],
            "peer must converge on the owner's set_tags delta"
        );
    }

    #[test]
    fn set_title_generates_update() {
        let mut node = KbNodeDoc::new("n1", "Old Title", "", &[]);
        let update = node.set_title("New Title");
        assert!(!update.is_empty());
        assert_eq!(node.title(), "New Title");
    }

    #[test]
    fn set_body_generates_update() {
        let mut node = KbNodeDoc::new("n1", "T", "old body", &[]);
        let update = node.set_body("new body content");
        assert!(!update.is_empty());
        assert_eq!(node.body(), "new body content");
    }

    #[test]
    fn tag_operations() {
        let mut node = KbNodeDoc::new("n1", "T", "", &["a".to_string()]);
        assert_eq!(node.tags(), vec!["a"]);

        node.add_tag("b");
        assert_eq!(node.tags(), vec!["a", "b"]);

        node.remove_tag("a");
        assert_eq!(node.tags(), vec!["b"]);
    }

    #[test]
    fn two_clients_merge_body() {
        let mut node_a = KbNodeDoc::new("n1", "T", "hello", &[]);
        let state = node_a.encode();

        let mut node_b = KbNodeDoc::from_bytes(&state).unwrap();
        assert_eq!(node_b.body(), "hello");

        // Both edit body (set_body replaces, so last-write-wins semantics)
        let update_a = node_a.set_body("from A");
        let update_b = node_b.set_body("from B");

        node_a.apply_update(&update_b).unwrap();
        node_b.apply_update(&update_a).unwrap();

        // Both converge to the same result
        assert_eq!(node_a.body(), node_b.body());
    }

    #[test]
    fn encode_decode_roundtrip() {
        let node = KbNodeDoc::new(
            "concept:arch",
            "Architecture",
            "The system uses...",
            &["core".to_string(), "design".to_string()],
        );
        let bytes = node.encode();

        let restored = KbNodeDoc::from_bytes(&bytes).unwrap();
        assert_eq!(restored.id(), "concept:arch");
        assert_eq!(restored.title(), "Architecture");
        assert_eq!(restored.body(), "The system uses...");
        assert_eq!(restored.tags(), vec!["core", "design"]);
    }

    // --- UTF-16 offset tests ---

    #[test]
    fn utf16_offset_cjk_roundtrip() {
        let node = KbNodeDoc::new("n1", "CJK", "", &[]);
        // CJK characters are multi-byte in UTF-8 but single code unit in UTF-16 (BMP)
        let mut n = KbNodeDoc::from_bytes(&node.encode()).unwrap();
        n.set_body("Hello 世界 and more text after");
        let bytes = n.encode();
        let restored = KbNodeDoc::from_bytes(&bytes).unwrap();
        assert_eq!(restored.body(), "Hello 世界 and more text after");
    }

    #[test]
    fn utf16_offset_emoji_roundtrip() {
        // Emoji above BMP (U+1F600) are 2 UTF-16 code units (surrogate pairs)
        let mut node = KbNodeDoc::new("n1", "Emoji Test 😀", "Body with 🎉 emoji", &[]);
        node.set_title("Updated 🌍 title");
        let bytes = node.encode();
        let restored = KbNodeDoc::from_bytes(&bytes).unwrap();
        assert_eq!(restored.title(), "Updated 🌍 title");
        assert_eq!(restored.body(), "Body with 🎉 emoji");
    }

    #[test]
    fn utf16_two_client_cjk_merge() {
        let mut node_a = KbNodeDoc::new_with_client_id("n1", "T", "你好", &[], 1);
        let state = node_a.encode();
        let mut node_b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();

        let update_a = node_a.set_body("你好世界");
        let update_b = node_b.set_body("你好朋友");

        node_a.apply_update(&update_b).unwrap();
        node_b.apply_update(&update_a).unwrap();

        assert_eq!(node_a.body(), node_b.body());
    }

    // --- Client ID tests ---

    #[test]
    fn new_with_client_id_preserves_identity() {
        let node = KbNodeDoc::new_with_client_id("n1", "T", "B", &[], 42);
        assert_eq!(node.id(), "n1");
        assert_eq!(node.title(), "T");
        // Verify client_id is set on the yrs Doc
        assert_eq!(node.doc().client_id().get(), 42);
    }

    #[test]
    fn from_bytes_with_client_id_preserves_identity() {
        let original = KbNodeDoc::new_with_client_id("n1", "T", "B", &[], 10);
        let bytes = original.encode();
        let restored = KbNodeDoc::from_bytes_with_client_id(&bytes, 20).unwrap();
        assert_eq!(restored.id(), "n1");
        assert_eq!(restored.doc().client_id().get(), 20);
    }

    // --- encode_diff tests ---

    #[test]
    fn encode_diff_produces_valid_update() {
        let mut node = KbNodeDoc::new("n1", "T", "hello", &[]);
        let sv_before = node.state_vector();
        node.set_body("hello world");
        let diff = node.encode_diff(&sv_before).unwrap();
        assert!(!diff.is_empty());

        // Apply the diff to a copy from before the change
        let mut old = KbNodeDoc::from_bytes(&{
            let orig = KbNodeDoc::new("n1", "T", "hello", &[]);
            orig.encode()
        })
        .unwrap();
        old.apply_update(&diff).unwrap();
        // After applying diff, old should have "hello world"
        // (The diff contains the set_body which replaces the entire text)
        assert!(old.body().contains("hello"));
    }

    // --- materialize tests ---

    #[test]
    fn materialize_extracts_all_fields() {
        let mut node = KbNodeDoc::new(
            "concept:test",
            "Test",
            "Body",
            &["tag1".to_string(), "tag2".to_string()],
        );
        node.add_link("concept:other");
        let mat = node.materialize();
        assert_eq!(mat.id, "concept:test");
        assert_eq!(mat.title, "Test");
        assert_eq!(mat.body, "Body");
        assert_eq!(mat.tags, vec!["tag1", "tag2"]);
        assert_eq!(mat.links, vec!["concept:other"]);
    }

    // --- content_hash tests ---

    #[test]
    fn content_hash_changes_on_edit() {
        let mut node = KbNodeDoc::new("n1", "T", "hello", &[]);
        let hash1 = node.content_hash();
        node.set_body("world");
        let hash2 = node.content_hash();
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn content_hash_stable_for_same_content() {
        let node1 = KbNodeDoc::new("n1", "T", "hello", &["a".to_string()]);
        let node2 = KbNodeDoc::new("n1", "T", "hello", &["a".to_string()]);
        assert_eq!(node1.content_hash(), node2.content_hash());
    }

    // --- apply_update returns changed flag ---

    #[test]
    fn apply_update_returns_changed_flag() {
        let mut node_a = KbNodeDoc::new_with_client_id("n1", "T", "hello", &[], 1);
        let state = node_a.encode();
        let mut node_b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();

        let update = node_b.set_body("changed");
        let changed = node_a.apply_update(&update).unwrap();
        assert!(changed, "content changed, flag should be true");

        // Apply same update again — no content change
        // (yrs deduplicates, so the flag should be false)
        let update2 = node_b.set_body("changed"); // no-op — same content
        let changed2 = node_a.apply_update(&update2).unwrap();
        // The body is still "changed" so hash should match
        assert!(!changed2, "same content, flag should be false");
    }

    // --- 3-client convergence ---

    #[test]
    fn three_client_concurrent_edits_converge() {
        let mut a = KbNodeDoc::new_with_client_id("n1", "T", "base", &[], 1);
        let state = a.encode();
        let mut b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();
        let mut c = KbNodeDoc::from_bytes_with_client_id(&state, 3).unwrap();

        // All three concurrently edit different fields
        let u_a = a.set_title("Title from A");
        let u_b = b.add_tag("tag-from-b");
        let u_c = c.add_link("link-from-c");

        // Apply all updates to all clients
        a.apply_update(&u_b).unwrap();
        a.apply_update(&u_c).unwrap();
        b.apply_update(&u_a).unwrap();
        b.apply_update(&u_c).unwrap();
        c.apply_update(&u_a).unwrap();
        c.apply_update(&u_b).unwrap();

        // All three should converge
        assert_eq!(a.title(), b.title());
        assert_eq!(b.title(), c.title());
        assert_eq!(a.title(), "Title from A");
        assert_eq!(a.tags(), b.tags());
        assert_eq!(b.tags(), c.tags());
        assert!(a.tags().contains(&"tag-from-b".to_string()));
        assert_eq!(a.links(), b.links());
        assert_eq!(b.links(), c.links());
        assert!(a.links().contains(&"link-from-c".to_string()));
    }

    // --- Multi-field concurrent edits ---

    #[test]
    fn concurrent_title_and_body_edits() {
        let mut a = KbNodeDoc::new_with_client_id("n1", "T", "B", &[], 1);
        let state = a.encode();
        let mut b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();

        let u_a = a.set_title("New Title");
        let u_b = b.set_body("New Body");

        a.apply_update(&u_b).unwrap();
        b.apply_update(&u_a).unwrap();

        assert_eq!(a.title(), "New Title");
        assert_eq!(a.body(), "New Body");
        assert_eq!(a.title(), b.title());
        assert_eq!(a.body(), b.body());
    }

    // --- Link and meta operations ---

    #[test]
    fn link_operations() {
        let mut node = KbNodeDoc::new("n1", "T", "", &[]);
        node.add_link("target1");
        node.add_link("target2");
        assert_eq!(node.links(), vec!["target1", "target2"]);

        node.remove_link("target1");
        assert_eq!(node.links(), vec!["target2"]);
    }

    #[test]
    fn meta_operations() {
        let mut node = KbNodeDoc::new("n1", "T", "", &[]);
        node.set_meta("author", "alice");
        node.set_meta("version", "2");
        assert_eq!(node.get_meta("author"), Some("alice".to_string()));
        assert_eq!(node.get_meta("version"), Some("2".to_string()));
        assert_eq!(node.get_meta("missing"), None);
    }

    // --- KbCollectionDoc tests ---

    #[test]
    fn collection_basic_creation() {
        let coll = KbCollectionDoc::new("Research Notes", "alice");
        assert_eq!(coll.name(), "Research Notes");
        assert_eq!(coll.creator(), "alice");
        assert_eq!(coll.members(), vec!["alice"]);
        assert_eq!(coll.node_count(), 0);
    }

    #[test]
    fn collection_add_remove_nodes() {
        let mut coll = KbCollectionDoc::new("Test", "alice");
        coll.add_node("concept:buffer", "Buffer");
        coll.add_node("concept:window", "Window");
        assert_eq!(coll.node_count(), 2);

        let nodes = coll.list_nodes();
        assert!(nodes.iter().any(|(id, _)| id == "concept:buffer"));
        assert!(nodes.iter().any(|(id, _)| id == "concept:window"));

        coll.remove_node("concept:buffer");
        assert_eq!(coll.node_count(), 1);
    }

    /// #156 F5: the enable-time manifest-title scrub. Blanks every cleartext title in
    /// ONE delta, preserves the node ids, leaves already-blank titles alone, and is
    /// idempotent (a second call has nothing to do → empty delta). The delta, applied to
    /// a fresh replica, reproduces the blanked manifest (round-trip).
    #[test]
    fn blank_node_titles_delta_scrubs_manifest_and_is_idempotent() {
        let mut coll = KbCollectionDoc::new("Test", "alice");
        coll.add_node("concept:a", "Secret Alpha");
        coll.add_node("concept:b", "Secret Beta");
        coll.add_node("concept:c", ""); // already blank
        assert!(coll.list_nodes().iter().any(|(_, t)| t == "Secret Alpha"));

        // A replica that SHARES this collection's lineage (built from its state — the
        // daemon applies the delta to the same `kbc:` doc, never an independent rebuild,
        // which would mint a divergent yrs client_id that wouldn't merge — the #179 rule).
        let mut replica = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();

        let delta = coll.blank_node_titles_delta();
        assert!(!delta.is_empty(), "produced a blanking delta");

        let nodes = coll.list_nodes();
        assert_eq!(nodes.len(), 3, "node ids preserved (only titles blanked)");
        assert!(
            nodes.iter().all(|(_, t)| t.is_empty()),
            "every manifest title is blank after the scrub: {nodes:?}"
        );

        // Idempotent — nothing left to blank.
        assert!(
            coll.blank_node_titles_delta().is_empty(),
            "second scrub is a no-op (empty delta)"
        );

        // The delta is a real applicable collection update: the lineage-sharing replica
        // replays it and sees the blanked manifest (round-trip), not the cleartext titles.
        replica.apply_update(&delta).unwrap();
        assert!(
            replica.list_nodes().iter().all(|(_, t)| t.is_empty()),
            "applying the delta blanks the titles on a replica too: {:?}",
            replica.list_nodes()
        );
    }

    /// #156 F5 — the AT-REST oracle (the attacker's test). Blanking the manifest title
    /// must not leave the ORIGINAL cleartext title recoverable in the `kbc:` doc's
    /// persisted state bytes (a yrs overwrite can keep the old value as a tombstone). The
    /// daemon stores `encode_state()`, so an attacker greps exactly these bytes.
    #[test]
    fn blank_node_titles_delta_purges_old_title_from_state_bytes() {
        let canary = b"SECRET-TITLE-CANARY-do-not-survive";
        let mut coll = KbCollectionDoc::new("Test", "alice");
        coll.add_node("concept:a", std::str::from_utf8(canary).unwrap());
        // Precondition (non-vacuous): the cleartext title IS in the state before scrub.
        assert!(
            coll.encode_state()
                .windows(canary.len())
                .any(|w| w == canary),
            "precondition: the title is in the state before blanking (else the test is vacuous)"
        );

        coll.blank_node_titles_delta();

        assert!(
            !coll
                .encode_state()
                .windows(canary.len())
                .any(|w| w == canary),
            "the original cleartext title MUST NOT survive in the kbc: state bytes after blanking"
        );
    }

    #[test]
    fn collection_members() {
        let mut coll = KbCollectionDoc::new("Test", "alice");
        coll.add_member("bob");
        coll.add_member("bob"); // duplicate — should not be added
        assert_eq!(coll.members(), vec!["alice", "bob"]);

        coll.remove_member("alice");
        assert_eq!(coll.members(), vec!["bob"]);
    }

    #[test]
    fn collection_set_creator_restamps_and_seeds_member() {
        // A collection built with a client-claimed creator...
        let mut coll = KbCollectionDoc::new("Test", "client-name");
        assert_eq!(coll.creator(), "client-name");
        // ...is re-stamped to the authenticated identity, which becomes a member.
        coll.set_creator("alice");
        assert_eq!(coll.creator(), "alice", "creator overridden");
        assert!(
            coll.members().contains(&"alice".to_string()),
            "creator seeded as member"
        );
        // Idempotent: no duplicate member on re-stamp.
        coll.set_creator("alice");
        assert_eq!(
            coll.members().iter().filter(|m| *m == "alice").count(),
            1,
            "no duplicate member"
        );
    }

    // ---- ADR-023 (B-19) epoch-fenced-rebase primitives ----

    #[test]
    fn member_epoch_advances_only_on_role_change() {
        // The daemon authors these ops, so the epoch is unforgeable by the client.
        let mut coll = KbCollectionDoc::new("Test", "alice");
        assert_eq!(coll.epoch_of("bob"), 0, "non-member starts at epoch 0");

        // Fresh grants stay at epoch 0 — no prior write-capable lineage to fence,
        // so owners + directly-added editors need no editor-side epoch sync.
        coll.set_owner("alice", "alice");
        assert_eq!(coll.epoch_of("alice"), 0, "fresh owner seeds at epoch 0");
        coll.upsert_member("bob", "bob", Role::Viewer);
        assert_eq!(coll.epoch_of("bob"), 0, "first grant ⇒ epoch 0");

        // Re-stamping the SAME role must not advance (B-12 owner re-share idempotency).
        coll.set_owner("alice", "alice");
        assert_eq!(coll.epoch_of("alice"), 0, "owner re-stamp preserves epoch");
        coll.upsert_member("bob", "bob", Role::Viewer);
        assert_eq!(
            coll.epoch_of("bob"),
            0,
            "same-role re-assignment is a no-op"
        );

        // The B-19 cascade vector: a role CHANGE. The epoch MUST advance so bob's
        // post-grant client_id differs from his viewer-era one — to an UNPREDICTABLE
        // token (#72), never the guessable prev+1.
        let viewer_epoch = coll.epoch_of("bob"); // 0
        coll.set_role("bob", Role::Editor);
        let editor_epoch = coll.epoch_of("bob");
        assert_ne!(
            editor_epoch, viewer_epoch,
            "viewer→editor advances the epoch"
        );
        assert_ne!(
            editor_epoch,
            viewer_epoch + 1,
            "advance is an unpredictable token, not prev+1 (#72)"
        );
        coll.upsert_member("bob", "bob", Role::Viewer);
        let reviewer_epoch = coll.epoch_of("bob");
        assert_ne!(reviewer_epoch, editor_epoch, "editor→viewer advances again");
        assert_ne!(
            reviewer_epoch, 0,
            "an advance never returns to the sentinel"
        );
    }

    #[test]
    fn derive_kb_client_id_rotates_with_epoch_and_stays_53bit() {
        let fp = "ed25519:AAAA";
        let ids: Vec<u64> = (0..4).map(|e| derive_kb_client_id(fp, e)).collect();
        // Distinct per epoch — a viewer-era op can never masquerade as current-epoch.
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(a, b, "epochs must yield distinct client_ids");
            }
        }
        // B-17: yrs ClientID is 53-bit; never 0/1.
        for id in &ids {
            assert!(*id < (1u64 << 53), "client_id must fit yrs' 53 bits");
            assert!(*id > 1, "client_id must avoid the reserved 0/1");
        }
        // Deterministic across the editor/daemon boundary.
        assert_eq!(derive_kb_client_id(fp, 2), derive_kb_client_id(fp, 2));
    }

    #[test]
    fn update_new_op_authors_flags_stale_epoch_lineage() {
        // The daemon's fence in miniature: an owner-authored node is the canonical
        // base; a viewer (old epoch) and a granted editor (new epoch) each author an
        // edit. update_new_op_authors must attribute each update to its real author,
        // so the daemon can reject the viewer-era lineage and accept only C_now.
        let fp = "ed25519:bob";
        let c_viewer = derive_kb_client_id(fp, 0); // pre-grant (added at epoch 0)
        let c_editor = derive_kb_client_id(fp, 1); // post-grant, viewer→editor (C_now)

        let base = KbNodeDoc::new_with_client_id("n1", "Original", "body", &[], 99);
        let base_state = base.encode_state();

        // Viewer-era edit (would be denied live, but lands in the local lineage).
        let mut viewer = KbNodeDoc::from_bytes_with_client_id(&base_state, c_viewer).unwrap();
        let viewer_update = viewer.set_title("hijacked");
        let viewer_authors = update_new_op_authors(&viewer_update, &base_state).unwrap();
        assert_eq!(
            viewer_authors,
            vec![c_viewer],
            "stale lineage is attributable"
        );
        assert!(
            !viewer_authors.iter().all(|a| *a == c_editor),
            "fence rejects: not every new op is from C_now"
        );

        // A fresh, current-epoch edit is accepted (every new op is C_now).
        let mut editor = KbNodeDoc::from_bytes_with_client_id(&base_state, c_editor).unwrap();
        let editor_update = editor.set_title("legit edit");
        let editor_authors = update_new_op_authors(&editor_update, &base_state).unwrap();
        assert_eq!(editor_authors, vec![c_editor]);
        assert!(
            editor_authors.iter().all(|a| *a == c_editor),
            "fence accepts: all new ops authored under C_now"
        );

        // Grandfathering: re-presenting only already-canonical ops flags no author.
        let empty = update_new_op_authors(&base_state, &base_state).unwrap();
        assert!(empty.is_empty(), "ops the daemon already has are not 'new'");
    }

    /// B-20 regression: a stale-epoch op that is a *contiguous-clock continuation*
    /// of a client already present in the canonical base must still be fenced.
    ///
    /// Live 9c: bob (editor, epoch 2) makes an accepted edit, so his epoch-2 client
    /// becomes canonical. He is demoted to viewer then re-promoted to editor (epoch
    /// jumps to 4), but his editor never rotated off the epoch-2 client (no rejoin),
    /// so a viewer-interval edit rides that *still-canonical* client. Because the
    /// op merely extends an existing lineage, the incoming update's own state vector
    /// omits it — the pre-fix fence saw "no new authors" and let it cascade. The
    /// fix integrates the update against the authoritative state and catches the
    /// clock advance.
    #[test]
    fn update_new_op_authors_flags_contiguous_stale_continuation() {
        let fp = "ed25519:bob";
        let c_e2 = derive_kb_client_id(fp, 2); // canonical via an accepted edit (9b)
        let c_now = derive_kb_client_id(fp, 4); // current epoch after demote->promote

        // Owner seeds the node; bob (epoch 2) makes the accepted edit -> the daemon's
        // authoritative state now contains bob's epoch-2 client.
        let owner = KbNodeDoc::new_with_client_id("n", "Original", "body", &[], 999_111);
        let mut bob = KbNodeDoc::from_bytes_with_client_id(&owner.encode_state(), c_e2).unwrap();
        let accepted = bob.set_title("POST-GRANT-EDIT");
        let mut daemon = KbNodeDoc::from_bytes(&owner.encode_state()).unwrap();
        daemon.apply_update(&accepted).unwrap();
        let base_state = daemon.encode_state(); // authoritative state the fence sees

        // bob (still epoch 2) appends a viewer-interval edit -> contiguous extension.
        let stale_update = bob.set_title("VIEWER-ERA");
        let authors = update_new_op_authors(&stale_update, &base_state).unwrap();
        assert!(
            authors.contains(&c_e2),
            "the contiguous stale-epoch continuation must be attributable (B-20)"
        );
        assert!(
            authors.iter().any(|a| *a != c_now),
            "fence MUST reject: a stale-epoch (c_e2) author is present though c_now is epoch 4"
        );
    }

    #[test]
    fn collection_encode_decode_roundtrip() {
        let mut coll = KbCollectionDoc::new("KB1", "alice");
        coll.add_node("n1", "Node One");
        coll.add_member("bob");

        let bytes = coll.encode_state();
        let restored = KbCollectionDoc::from_bytes(&bytes).unwrap();
        assert_eq!(restored.name(), "KB1");
        assert_eq!(restored.creator(), "alice");
        assert_eq!(restored.node_count(), 1);
        assert_eq!(restored.members().len(), 2);
    }

    #[test]
    fn collection_two_client_merge() {
        let mut coll_a = KbCollectionDoc::new_with_client_id("KB1", "alice", 1);
        let state = coll_a.encode_state();
        let mut coll_b = KbCollectionDoc::from_bytes(&state).unwrap();

        let u_a = coll_a.add_node("n1", "From A");
        let u_b = coll_b.add_node("n2", "From B");

        coll_a.apply_update(&u_b).unwrap();
        coll_b.apply_update(&u_a).unwrap();

        assert_eq!(coll_a.node_count(), 2);
        assert_eq!(coll_b.node_count(), 2);

        let nodes_a = coll_a.list_nodes();
        let nodes_b = coll_b.list_nodes();
        assert_eq!(nodes_a.len(), nodes_b.len());
    }

    // --- ADR-018 v2 schema: owner / roles / policy / pending ---

    #[test]
    fn role_hierarchy_includes() {
        assert!(Role::Owner.includes(Role::Editor));
        assert!(Role::Owner.includes(Role::Viewer));
        assert!(Role::Editor.includes(Role::Viewer));
        assert!(!Role::Viewer.includes(Role::Editor));
        assert!(!Role::Editor.includes(Role::Owner));
    }

    #[test]
    fn collection_v2_new_owned_seeds_owner_role_policy() {
        let coll = KbCollectionDoc::new_owned("KB", "SHA256:owner", "alice");
        assert_eq!(coll.schema_version(), 2);
        assert_eq!(coll.owner(), "SHA256:owner");
        assert_eq!(coll.owner_label(), "alice");
        assert_eq!(coll.role_of("SHA256:owner"), Some(Role::Owner));
        assert_eq!(coll.join_policy(), JoinPolicy::Invite);
        assert!(coll.pending().is_empty());
        assert_eq!(coll.member_roles().len(), 1);
    }

    #[test]
    fn collection_v2_roles_and_upsert() {
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:owner", "alice");
        coll.upsert_member("SHA256:bob", "bob", Role::Editor);
        assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Editor));
        coll.set_role("SHA256:bob", Role::Viewer);
        assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Viewer));
        coll.remove_principal("SHA256:bob");
        assert_eq!(coll.role_of("SHA256:bob"), None);
    }

    // --- #72 epoch-fence hardening (security-negative oracles) ---

    #[test]
    fn epoch_advance_is_not_predictable_counter() {
        // Pre-rotation defense (ADR-023): a role change must NOT advance the epoch
        // to a guessable prev+1, or an attacker precomputes derive(fp, prev+1) and
        // authors viewer-era ops under the future editor client_id.
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:owner", "alice");
        coll.upsert_member("SHA256:bob", "bob", Role::Viewer); // fresh grant -> epoch 0
        let prev = coll.epoch_of("SHA256:bob");
        let predicted = derive_kb_client_id("SHA256:bob", prev + 1);
        coll.set_role("SHA256:bob", Role::Editor); // role change -> epoch advance
        let actual = derive_kb_client_id("SHA256:bob", coll.epoch_of("SHA256:bob"));
        assert_ne!(
            predicted, actual,
            "epoch advance must be an unpredictable token, not prev+1"
        );
    }

    #[test]
    fn readd_after_remove_does_not_reuse_clientid() {
        // Monotonicity across remove/re-add (ADR-023): a directly-added editor
        // authors under derive(fp, 0). If remove+re-add resets to epoch 0, their
        // pre-removal lineage is silently un-fenced. The re-added member's
        // authoring client_id MUST differ from the pre-removal one.
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:owner", "alice");
        coll.upsert_member("SHA256:bob", "bob", Role::Editor); // fresh grant -> epoch 0
        let before = derive_kb_client_id("SHA256:bob", coll.epoch_of("SHA256:bob"));
        coll.remove_principal("SHA256:bob");
        coll.upsert_member("SHA256:bob", "bob", Role::Editor); // re-add
        let after = derive_kb_client_id("SHA256:bob", coll.epoch_of("SHA256:bob"));
        assert_ne!(
            before, after,
            "re-add must issue a fresh epoch, not reuse the pre-removal client_id"
        );
    }

    #[test]
    fn collection_v2_join_policy() {
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
        assert_eq!(coll.join_policy(), JoinPolicy::Invite);
        coll.set_join_policy(JoinPolicy::Restrictive);
        assert_eq!(coll.join_policy(), JoinPolicy::Restrictive);
    }

    #[test]
    fn transport_policy_logic() {
        // Round-trip.
        for p in [
            TransportPolicy::Hub,
            TransportPolicy::P2p,
            TransportPolicy::Both,
        ] {
            assert_eq!(TransportPolicy::parse(p.as_str()), Some(p));
        }
        assert_eq!(TransportPolicy::parse("nonsense"), None);

        // allows(): the exposure matrix.
        assert!(TransportPolicy::Hub.allows(Transport::Hub));
        assert!(!TransportPolicy::Hub.allows(Transport::P2p));
        assert!(TransportPolicy::P2p.allows(Transport::P2p));
        assert!(!TransportPolicy::P2p.allows(Transport::Hub));
        assert!(TransportPolicy::Both.allows(Transport::Hub));
        assert!(TransportPolicy::Both.allows(Transport::P2p));

        // with(): widening is idempotent; mixing transports ⇒ Both.
        assert_eq!(
            TransportPolicy::Hub.with(Transport::Hub),
            TransportPolicy::Hub
        );
        assert_eq!(
            TransportPolicy::Hub.with(Transport::P2p),
            TransportPolicy::Both
        );
        assert_eq!(
            TransportPolicy::P2p.with(Transport::Hub),
            TransportPolicy::Both
        );
        assert_eq!(
            TransportPolicy::Both.with(Transport::Hub),
            TransportPolicy::Both
        );
    }

    #[test]
    fn collection_transport_policy_defaults_to_hub() {
        // Conservative default: a freshly-shared (or pre-feature) KB is Hub-only —
        // NOT exposed to the mesh until explicitly p2p-shared.
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
        assert_eq!(coll.transport_policy(), TransportPolicy::Hub);
        assert!(coll.transport_policy().allows(Transport::Hub));
        assert!(!coll.transport_policy().allows(Transport::P2p));

        // Opt into the mesh.
        coll.set_transport_policy(TransportPolicy::Both);
        assert_eq!(coll.transport_policy(), TransportPolicy::Both);
        assert!(coll.transport_policy().allows(Transport::P2p));
    }

    #[test]
    fn collection_encryption_defaults_to_none_and_round_trips() {
        // ADR-037: a pre-feature / freshly-shared KB is plaintext (absent flag), so
        // v0.14 KBs are unchanged; the owner can opt a KB into E2E.
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
        assert_eq!(coll.encryption(), Encryption::None, "absent flag ⇒ None");
        coll.set_encryption(Encryption::E2e);
        assert_eq!(coll.encryption(), Encryption::E2e, "round-trips E2e");
        assert_eq!(Encryption::parse("e2e"), Some(Encryption::E2e));
        assert_eq!(Encryption::parse("bogus"), None);
    }

    #[test]
    fn transport_policy_raw_and_union_widening() {
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
        // Never set ⇒ raw None (distinct from an explicit Hub), effective Hub.
        assert_eq!(coll.transport_policy_raw(), None);
        assert_eq!(coll.transport_policy(), TransportPolicy::Hub);

        // First share over p2p ⇒ P2p-only (set, not unioned with the Hub default).
        coll.set_transport_policy(TransportPolicy::P2p);
        assert_eq!(coll.transport_policy_raw(), Some(TransportPolicy::P2p));

        // A later hub re-share widens P2p ∪ Hub ⇒ Both.
        let widened = coll.transport_policy().union(TransportPolicy::Hub);
        assert_eq!(widened, TransportPolicy::Both);

        // union algebra.
        assert_eq!(
            TransportPolicy::Hub.union(TransportPolicy::Hub),
            TransportPolicy::Hub
        );
        assert_eq!(
            TransportPolicy::Both.union(TransportPolicy::P2p),
            TransportPolicy::Both
        );
    }

    #[test]
    fn collection_v2_pending_then_approve_atomic() {
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
        coll.add_pending("SHA256:bob", "bob", "2026-06-16T00:00:00Z", None, None);
        assert_eq!(coll.pending().len(), 1);
        assert_eq!(coll.role_of("SHA256:bob"), None);
        coll.approve("SHA256:bob", Role::Editor);
        assert!(coll.pending().is_empty(), "approve clears pending");
        assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Editor));
        let m = coll
            .member_roles()
            .into_iter()
            .find(|m| m.fingerprint == "SHA256:bob")
            .unwrap();
        assert_eq!(m.label, "bob", "approve carries the pending label");
    }

    #[test]
    fn add_pending_round_trips_the_joiner_pubkey() {
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
        // With a pubkey: pending() recovers it, so the owner can wrap_to_member on approve.
        let pk = [42u8; 32];
        coll.add_pending("SHA256:bob", "bob", "t", Some(&pk), None);
        let bob = coll
            .pending()
            .into_iter()
            .find(|p| p.fingerprint == "SHA256:bob")
            .unwrap();
        assert_eq!(
            bob.pubkey,
            Some(pk),
            "the joiner's pubkey round-trips through the pending record"
        );
        // Without a pubkey (a v1 record): reads back None (backward-compatible).
        coll.add_pending("SHA256:carol", "carol", "t", None, None);
        let carol = coll
            .pending()
            .into_iter()
            .find(|p| p.fingerprint == "SHA256:carol")
            .unwrap();
        assert_eq!(
            carol.pubkey, None,
            "a pubkey-less pending record reads back None"
        );
    }

    #[test]
    fn collection_v2_two_client_member_merge_converges() {
        let mut a =
            KbCollectionDoc::new_owned_with("KB", "SHA256:o", "alice", Some(1), JoinPolicy::Invite);
        let state = a.encode_state();
        let mut b = KbCollectionDoc::from_bytes(&state).unwrap();
        let ua = a.upsert_member("SHA256:bob", "bob", Role::Editor);
        let ub = b.upsert_member("SHA256:carol", "carol", Role::Viewer);
        a.apply_update(&ub).unwrap();
        b.apply_update(&ua).unwrap();
        for c in [&a, &b] {
            assert_eq!(c.role_of("SHA256:bob"), Some(Role::Editor));
            assert_eq!(c.role_of("SHA256:carol"), Some(Role::Viewer));
        }
        assert_eq!(a.member_roles().len(), b.member_roles().len());
    }

    #[test]
    fn collection_v2_roundtrip_preserves_schema() {
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
        coll.upsert_member("SHA256:bob", "bob", Role::Viewer);
        coll.set_join_policy(JoinPolicy::Permissive);
        coll.add_pending("SHA256:eve", "eve", "t", None, None);
        let bytes = coll.encode_state();
        let r = KbCollectionDoc::from_bytes(&bytes).unwrap();
        assert_eq!(r.schema_version(), 2);
        assert_eq!(r.owner(), "SHA256:o");
        assert_eq!(r.role_of("SHA256:bob"), Some(Role::Viewer));
        assert_eq!(r.join_policy(), JoinPolicy::Permissive);
        assert_eq!(r.pending().len(), 1);
    }

    #[test]
    fn migrate_v1_resolves_labels_to_principals() {
        // Build a legacy v1 collection (label creator + members YArray).
        let mut coll = KbCollectionDoc::new("KB", "alice");
        coll.add_member("bob");
        assert_eq!(coll.schema_version(), 0, "legacy = no schema key");
        // resolver maps known labels to fingerprints.
        let resolver = |label: &str| match label {
            "alice" => Some(("SHA256:alice".to_string(), "alice".to_string())),
            "bob" => Some(("SHA256:bob".to_string(), "bob".to_string())),
            _ => None,
        };
        let update = coll.migrate_if_legacy(resolver).expect("migrated");
        assert!(!update.is_empty());
        assert_eq!(coll.schema_version(), 2);
        assert_eq!(coll.owner(), "SHA256:alice");
        assert_eq!(coll.role_of("SHA256:alice"), Some(Role::Owner));
        assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Editor));
        assert_eq!(coll.join_policy(), JoinPolicy::Invite);
        // idempotent
        assert!(coll.migrate_if_legacy(resolver).is_none());
    }

    #[test]
    fn migrate_v1_unresolved_label_falls_back_to_legacy_principal() {
        let mut coll = KbCollectionDoc::new("KB", "alice");
        coll.add_member("ghost");
        // resolver knows nobody → legacy:<label> principals.
        coll.migrate_if_legacy(|_| None).expect("migrated");
        assert_eq!(coll.schema_version(), 2);
        assert_eq!(coll.owner(), "legacy:alice");
        assert_eq!(coll.role_of("legacy:alice"), Some(Role::Owner));
        assert_eq!(coll.role_of("legacy:ghost"), Some(Role::Editor));
    }

    // --- ADR-026 signed membership op-log (slice 2b-2) ---

    /// (secret seed, public key bytes, principal fingerprint) for a test identity.
    fn oplog_keypair(seed: u8) -> ([u8; 32], [u8; 32], String) {
        use crate::membership::fingerprint_of;
        use ed25519_dalek::SigningKey;
        let secret = [seed; 32];
        let pubkey = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let fp = fingerprint_of(&pubkey);
        (secret, pubkey, fp)
    }

    #[test]
    fn oplog_append_read_roundtrips_and_verifies() {
        let (secret, pubkey, owner_fp) = oplog_keypair(1);
        let mut coll = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");
        assert!(coll.oplog_head().is_none(), "empty log has no head");

        // Genesis: the owner admits themselves (self-signed).
        let op = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &owner_fp,
            Some(Role::Owner),
            true,
            &owner_fp,
            1000,
            None,
            0,
        );
        assert_eq!(op.prev_hash, "", "genesis op has empty prev_hash");
        let sig = op.sign(&secret);
        coll.append_signed_op(&op, &sig, &pubkey);

        let ops = coll.oplog_ops();
        assert_eq!(ops.len(), 1);
        let rec = &ops[0];
        assert!(rec.verify_signed(), "round-tripped record verifies");
        assert_eq!(rec.op.subject, owner_fp);
        assert_eq!(rec.op.role, Some(Role::Owner));
        assert!(rec.op.can_invite);
        assert_eq!(rec.op.kb_id, "KB");
        assert_eq!(
            coll.oplog_head(),
            Some(rec.chain_hash()),
            "head is the lone op"
        );
        // Re-appending the identical signed op is idempotent (keyed by chain_hash).
        coll.append_signed_op(&op, &sig, &pubkey);
        assert_eq!(
            coll.oplog_len(),
            1,
            "same op re-append is a no-op set insert"
        );
    }

    #[test]
    fn author_e2e_genesis_signs_encryption_self_wraps_and_relays_to_peers() {
        use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
        use crate::membership::{derive_content_key, derive_encryption};
        let (secret, pubkey, owner_fp) = oplog_keypair(1);
        let mut coll = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");
        assert_eq!(
            derive_encryption(&coll.oplog_ops(), &pubkey),
            Encryption::None,
            "unencrypted before enable"
        );

        let k = ContentKey::generate();
        let self_wrap = wrap_to_member(&k, &wrap_public_for(&secret)).unwrap();
        let delta = coll.author_e2e_genesis("KB", &owner_fp, &secret, &pubkey, self_wrap, 1000);

        // Authoritative mode is the SIGNED op-log; the unsigned flag mirrors it; and the
        // owner recovers its OWN self-wrapped content key from the log.
        assert_eq!(
            derive_encryption(&coll.oplog_ops(), &pubkey),
            Encryption::E2e,
            "signed SetEncryption(e2e) latched"
        );
        assert_eq!(
            coll.encryption(),
            Encryption::E2e,
            "unsigned flag set for display"
        );
        assert_eq!(
            derive_content_key(&coll.oplog_ops(), &pubkey, &owner_fp, &secret)
                .map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "owner recovers its self-wrapped key"
        );

        // A peer applying the shipped delta to a fresh replica derives the SAME signed
        // state (the daemon relays this delta key-blind).
        let mut peer = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");
        peer.apply_update(&delta).unwrap();
        assert_eq!(
            derive_encryption(&peer.oplog_ops(), &pubkey),
            Encryption::E2e,
            "peer derives e2e from the relayed delta"
        );
        assert_eq!(
            derive_content_key(&peer.oplog_ops(), &pubkey, &owner_fp, &secret)
                .map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "the owner on a fresh replica still recovers the key"
        );

        // A different identity (a non-member) recovers nothing — no wrap targets them.
        let (other_secret, _other_pubkey, other_fp) = oplog_keypair(2);
        assert!(
            derive_content_key(&coll.oplog_ops(), &pubkey, &other_fp, &other_secret).is_none(),
            "a non-member recovers no content key"
        );

        // Idempotent: a second enable on the already-genesis'd KB adds no second genesis.
        let len_before = coll.oplog_len();
        let k2_wrap = wrap_to_member(&k, &wrap_public_for(&secret)).unwrap();
        coll.author_e2e_genesis("KB", &owner_fp, &secret, &pubkey, k2_wrap, 1001);
        assert!(coll.oplog_len() >= len_before, "re-enable never DROPS ops");
        assert_eq!(
            derive_encryption(&coll.oplog_ops(), &pubkey),
            Encryption::E2e,
            "still e2e after re-enable"
        );
    }

    #[test]
    fn author_member_admit_delivers_the_key_stores_pubkey_and_keeps_epoch_consistent() {
        use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
        use crate::membership::derive_content_key;
        let (osec, opk, ofp) = oplog_keypair(1);
        let (msec, mpk, mfp) = oplog_keypair(2);
        let (_xsec, _xpk, xfp) = oplog_keypair(3); // a non-member
        let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        let k = ContentKey::generate();
        // Owner enables (genesis + self-wrap).
        let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
        coll.author_e2e_genesis("KB", &ofp, &osec, &opk, self_wrap, 1000);

        // Owner admits a member, wrapping the content key to THEM.
        let member_wrap = wrap_to_member(&k, &wrap_public_for(&msec)).unwrap();
        let delta = coll.author_member_admit(
            "KB",
            &mfp,
            &mpk,
            &wrap_public_for(&msec),
            Role::Editor,
            "m",
            member_wrap,
            &ofp,
            &osec,
            &opk,
            1001,
        );

        // The member is now Editor, recovers the SAME content key, and their pubkey is
        // stored (for re-wrap on rotation, 3c).
        assert_eq!(coll.role_of(&mfp), Some(Role::Editor));
        assert_eq!(
            derive_content_key(&coll.oplog_ops(), &opk, &mfp, &msec).map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "the admitted member recovers the content key"
        );
        assert_eq!(
            coll.member_pubkey(&mfp),
            Some(mpk),
            "member pubkey stored for re-wrap"
        );

        // Epoch consistency (the dual-write): the member's signed Admit op carries the SAME
        // epoch as the member_roles entry the daemon's fence reads.
        let admit = coll
            .oplog_ops()
            .into_iter()
            .find(|o| o.op.subject == mfp && o.op.action == MembershipAction::Admit)
            .unwrap();
        assert_eq!(
            admit.op.epoch,
            coll.epoch_of(&mfp),
            "op epoch == member_roles epoch"
        );

        // A peer with the relayed collection agrees (the admit delta is incremental on
        // top of the genesis a peer already holds; a member with the full collection
        // derives the key). `delta` is non-empty (it carries the admit).
        assert!(
            !delta.is_empty(),
            "the admit produces a non-empty delta to ship"
        );
        let peer = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
        assert_eq!(
            derive_content_key(&peer.oplog_ops(), &opk, &mfp, &msec).map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "the member recovers the key on a peer replica too"
        );
        assert!(
            derive_content_key(&coll.oplog_ops(), &opk, &xfp, &_xsec).is_none(),
            "a non-member recovers no key"
        );
    }

    /// ADR-040 §1-3 end-to-end through the yrs doc: a member rotates (`author_rebind`,
    /// the v3 op survives serialization via `op_from_map`), the owner re-wraps the content
    /// key to the successor (`author_rebind_rewrap`), and a FRESH PEER replica derives the
    /// rotated membership + delivers the key to the new identity. Adversarial oracles:
    /// the predecessor is retired from membership; a non-member still recovers nothing.
    #[test]
    fn rebind_rotates_identity_and_owner_rewrap_delivers_key_through_a_peer_replica() {
        use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
        use crate::membership::{derive_content_key, derive_valid_members};
        let (osec, opk, ofp) = oplog_keypair(1);
        let (bsec, bpk, bfp) = oplog_keypair(2); // bob's OLD identity
        let (b2sec, b2pk, b2fp) = oplog_keypair(3); // bob's NEW identity
        let (_xsec, _xpk, xfp) = oplog_keypair(4); // a non-member
        let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        let k = ContentKey::generate();

        // Owner enables e2e + admits bob (Editor), wrapping the key to bob's OLD wrap key.
        let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
        coll.author_e2e_genesis("KB", &ofp, &osec, &opk, self_wrap, 1000);
        let bob_wrap = wrap_to_member(&k, &wrap_public_for(&bsec)).unwrap();
        coll.author_member_admit(
            "KB",
            &bfp,
            &bpk,
            &wrap_public_for(&bsec),
            Role::Editor,
            "bob",
            bob_wrap,
            &ofp,
            &osec,
            &opk,
            1001,
        );
        assert_eq!(coll.role_of(&bfp), Some(Role::Editor));

        // Bob rotates his identity: the OLD key cross-signs the NEW key.
        coll.author_rebind(
            "KB",
            &bfp,
            &b2fp,
            &b2pk,
            &wrap_public_for(&b2sec),
            &bsec,
            &bpk,
            1002,
        );
        // Membership transfers to the successor; the predecessor is retired.
        let m = derive_valid_members(&coll.oplog_ops(), &opk, 2000);
        assert!(!m.contains_key(&bfp), "predecessor retired");
        assert_eq!(
            m.get(&b2fp).map(|x| x.role),
            Some(Role::Editor),
            "successor inherits Editor"
        );
        // ...but the successor can't read yet (no wrap targets the new key).
        assert!(
            derive_content_key(&coll.oplog_ops(), &opk, &b2fp, &b2sec).is_none(),
            "successor cannot decrypt until the owner re-wraps"
        );

        // Owner observes the rebind and re-wraps the CURRENT key to the successor's wrap key.
        let succ_wrap = wrap_to_member(&k, &wrap_public_for(&b2sec)).unwrap();
        // Member rotation re-wrapped by the STABLE owner: anchor == signer == owner.
        coll.author_rebind_rewrap("KB", &b2fp, &b2pk, succ_wrap, &opk, &ofp, &osec, &opk, 1003);

        // The successor now recovers the SAME content key — proven on a FRESH PEER replica
        // (the v3 Rebind op + the re-wrap Admit both survived yrs serialization).
        let peer = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
        let pm = derive_valid_members(&peer.oplog_ops(), &opk, 2000);
        assert!(!pm.contains_key(&bfp), "peer agrees: predecessor retired");
        assert!(pm.contains_key(&b2fp), "peer agrees: successor present");
        assert_eq!(
            derive_content_key(&peer.oplog_ops(), &opk, &b2fp, &b2sec).map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "the rotated member recovers the content key on a peer replica"
        );
        // A non-member still recovers nothing (selective oracle).
        assert!(
            derive_content_key(&peer.oplog_ops(), &opk, &xfp, &_xsec).is_none(),
            "a non-member recovers no key after rotation"
        );
        // Planned-rotation property (ADR-040 threat model): the OLD key, still held by the
        // user during a planned rotation, retains read access to pre-rotation content (its
        // original wrap is untouched — only a §D3 rotation revokes that).
        assert_eq!(
            derive_content_key(&peer.oplog_ops(), &opk, &bfp, &bsec).map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "old key retains read access to history (planned rotation, not §D3 revocation)"
        );
    }

    /// ADR-040 §Recovery-key (the attacker's test): a member registers an offline recovery
    /// key R (signed by its primary), LOSES the primary, and rotates using R — honored. A
    /// FORGER who lacks R cannot rotate the member, even authoring the identical Rebind shape.
    #[test]
    fn recovery_key_signed_rebind_is_honored_and_forgery_is_rejected() {
        use crate::content_crypto::wrap_public_for;
        use crate::membership::derive_valid_members;
        let (osec, opk, ofp) = oplog_keypair(1);
        let (msec, mpk, mfp) = oplog_keypair(2); // member's primary
        let (rsec, rpk, _rfp) = oplog_keypair(3); // member's OFFLINE recovery key
        let (s2sec, s2pk, s2fp) = oplog_keypair(4); // the recovered successor
        let (zsec, zpk, _zfp) = oplog_keypair(9); // a forger's key (NOT R)
        let (g2sec, g2pk, g2fp) = oplog_keypair(10); // the forger's would-be successor

        let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        coll.author_e2e_genesis(
            "KB",
            &ofp,
            &osec,
            &opk,
            crate::content_crypto::wrap_to_member(
                &crate::content_crypto::ContentKey::generate(),
                &wrap_public_for(&osec),
            )
            .unwrap(),
            1000,
        );
        // Owner admits the member (Editor), no content wrap needed for this membership test.
        coll.author_member_admit(
            "KB",
            &mfp,
            &mpk,
            &wrap_public_for(&msec),
            Role::Editor,
            "m",
            crate::content_crypto::wrap_to_member(
                &crate::content_crypto::ContentKey::generate(),
                &wrap_public_for(&msec),
            )
            .unwrap(),
            &ofp,
            &osec,
            &opk,
            1001,
        );
        // The member registers its recovery key R (signed by its PRIMARY).
        coll.author_register_recovery_key("KB", &mfp, &rpk, &msec, &mpk, 1002);

        // PRIMARY LOST. The member rotates m → s2 using the RECOVERY key R.
        coll.author_recovery_rebind(
            "KB",
            &mfp,
            &s2fp,
            &s2pk,
            &wrap_public_for(&s2sec),
            &rsec,
            &rpk,
            1003,
        );

        // A FORGER, lacking R, authors the same-shaped recovery Rebind m → g2 with key Z.
        coll.author_recovery_rebind(
            "KB",
            &mfp,
            &g2fp,
            &g2pk,
            &wrap_public_for(&g2sec),
            &zsec,
            &zpk,
            1004,
        );

        // On a FRESH peer: the recovery-key rotation is honored (s2 inherits Editor, m retired);
        // the forged one is NOT (g2 is not a member — the registry binds recovery to R alone).
        let peer = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
        let m = derive_valid_members(&peer.oplog_ops(), &opk, 2000);
        assert_eq!(
            m.get(&s2fp).map(|x| x.role),
            Some(Role::Editor),
            "the recovery-key-signed rotation is honored; successor inherits Editor"
        );
        assert!(!m.contains_key(&mfp), "the recovered primary is retired");
        assert!(
            !m.contains_key(&g2fp),
            "a forger without the recovery key cannot rotate the member"
        );
    }

    /// ADR-040 §Recovery-key: with NO registration, a Rebind for a principal signed by any
    /// non-primary key is not honored — recovery requires a pre-registered key.
    #[test]
    fn recovery_rebind_without_registration_is_rejected() {
        use crate::content_crypto::wrap_public_for;
        use crate::membership::derive_valid_members;
        let (osec, opk, ofp) = oplog_keypair(1);
        let (_msec, _mpk, mfp) = oplog_keypair(2);
        let (rsec, rpk, _rfp) = oplog_keypair(3); // an UNREGISTERED key
        let (s2sec, s2pk, s2fp) = oplog_keypair(4);
        let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        let g = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &ofp,
            Some(Role::Owner),
            true,
            &ofp,
            1000,
            None,
            0,
        );
        let gs = g.sign(&osec);
        coll.append_signed_op(&g, &gs, &opk);
        let a = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &mfp,
            Some(Role::Editor),
            false,
            &ofp,
            1001,
            None,
            0,
        );
        let as_ = a.sign(&osec);
        coll.append_signed_op(&a, &as_, &opk);
        // No author_register_recovery_key. Try to recover m → s2 with an unregistered key.
        coll.author_recovery_rebind(
            "KB",
            &mfp,
            &s2fp,
            &s2pk,
            &wrap_public_for(&s2sec),
            &rsec,
            &rpk,
            1002,
        );
        let m = derive_valid_members(&coll.oplog_ops(), &opk, 2000);
        assert!(
            !m.contains_key(&s2fp),
            "no registration ⇒ recovery rebind ignored"
        );
        assert_eq!(
            m.get(&mfp).map(|x| x.role),
            Some(Role::Editor),
            "the member is unchanged"
        );
    }

    /// ADR-040 §Recovery-key: a registration is only honored when signed by the PRINCIPAL'S
    /// PRIMARY (verify_signed). A registration "for m" signed by an attacker's key never enters
    /// the registry, so a Rebind signed by that attacker key is not honored.
    #[test]
    fn recovery_registration_requires_the_primary() {
        use crate::content_crypto::wrap_public_for;
        use crate::membership::derive_valid_members;
        let (osec, opk, ofp) = oplog_keypair(1);
        let (_msec, _mpk, mfp) = oplog_keypair(2);
        let (atksec, atkpk, _atkfp) = oplog_keypair(8); // attacker key
        let (s2sec, s2pk, s2fp) = oplog_keypair(4);
        let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        let g = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &ofp,
            Some(Role::Owner),
            true,
            &ofp,
            1000,
            None,
            0,
        );
        let gs = g.sign(&osec);
        coll.append_signed_op(&g, &gs, &opk);
        let a = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &mfp,
            Some(Role::Editor),
            false,
            &ofp,
            1001,
            None,
            0,
        );
        let as_ = a.sign(&osec);
        coll.append_signed_op(&a, &as_, &opk);
        // Attacker forges a RegisterRecoveryKey for m, authorizing ITS OWN key as recovery —
        // but signs with the attacker key (not m's primary). subject=m, author=m, signed by atk.
        let mut reg = coll.build_membership_op(
            "KB",
            MembershipAction::RegisterRecoveryKey,
            &mfp,
            None,
            false,
            &mfp,
            1002,
            None,
            0,
        );
        reg.recovery_pubkey = Some(atkpk);
        let regsig = reg.sign(&atksec); // WRONG signer (not m's primary)
        coll.append_signed_op(&reg, &regsig, &atkpk);
        // Attacker now tries to recover m → s2 with its key.
        coll.author_recovery_rebind(
            "KB",
            &mfp,
            &s2fp,
            &s2pk,
            &wrap_public_for(&s2sec),
            &atksec,
            &atkpk,
            1003,
        );
        let m = derive_valid_members(&coll.oplog_ops(), &opk, 2000);
        assert!(
            !m.contains_key(&s2fp),
            "a registration not signed by the primary is ignored"
        );
    }

    /// ADR-040 §Recovery-key: latest registration wins (revoke a leaked recovery key). After
    /// R1 is superseded by R2, a Rebind signed by R1 is rejected while one signed by R2 is
    /// honored. Two independent collections (same op history up to the competing rebind) so the
    /// rejected branch doesn't causally orphan the honored one.
    #[test]
    fn latest_recovery_key_registration_wins() {
        use crate::content_crypto::wrap_public_for;
        use crate::membership::derive_valid_members;
        let (osec, opk, ofp) = oplog_keypair(1);
        let (msec, mpk, mfp) = oplog_keypair(2);
        let (r1sec, r1pk, _r1fp) = oplog_keypair(3); // first (leaked) recovery key
        let (r2sec, r2pk, _r2fp) = oplog_keypair(5); // replacement recovery key
        let (ssec, spk, sfp) = oplog_keypair(4); // the would-be successor

        // Build the shared prefix: owner genesis + admit m + register R1 + supersede with R2.
        let seed = |label: &str| {
            let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
            let g = coll.build_membership_op(
                "KB",
                MembershipAction::Admit,
                &ofp,
                Some(Role::Owner),
                true,
                &ofp,
                1000,
                None,
                0,
            );
            let gs = g.sign(&osec);
            coll.append_signed_op(&g, &gs, &opk);
            let a = coll.build_membership_op(
                "KB",
                MembershipAction::Admit,
                &mfp,
                Some(Role::Editor),
                false,
                &ofp,
                1001,
                None,
                0,
            );
            let as_ = a.sign(&osec);
            coll.append_signed_op(&a, &as_, &opk);
            coll.author_register_recovery_key("KB", &mfp, &r1pk, &msec, &mpk, 1002);
            coll.author_register_recovery_key("KB", &mfp, &r2pk, &msec, &mpk, 1003); // supersedes R1
            let _ = label;
            coll
        };

        // Branch A: rotate with the SUPERSEDED R1 → rejected (m unchanged).
        let mut a = seed("a");
        a.author_recovery_rebind(
            "KB",
            &mfp,
            &sfp,
            &spk,
            &wrap_public_for(&ssec),
            &r1sec,
            &r1pk,
            1004,
        );
        let ma = derive_valid_members(&a.oplog_ops(), &opk, 2000);
        assert!(
            !ma.contains_key(&sfp),
            "a Rebind signed by the SUPERSEDED recovery key is rejected"
        );
        assert_eq!(
            ma.get(&mfp).map(|x| x.role),
            Some(Role::Editor),
            "the member is unchanged under the revoked key"
        );

        // Branch B: rotate with the CURRENT R2 → honored (m → successor).
        let mut b = seed("b");
        b.author_recovery_rebind(
            "KB",
            &mfp,
            &sfp,
            &spk,
            &wrap_public_for(&ssec),
            &r2sec,
            &r2pk,
            1004,
        );
        let mb = derive_valid_members(&b.oplog_ops(), &opk, 2000);
        assert_eq!(
            mb.get(&sfp).map(|x| x.role),
            Some(Role::Editor),
            "the CURRENT recovery key rotates the member"
        );
        assert!(
            !mb.contains_key(&mfp),
            "and the recovered member key is retired"
        );
    }

    /// ADR-040 §Recovery-key: a REMOVED member cannot recover — `authorized`'s Rebind arm
    /// still requires the recovered principal be a current member, independent of who signed.
    #[test]
    fn removed_member_cannot_recover() {
        use crate::content_crypto::wrap_public_for;
        use crate::membership::derive_valid_members;
        let (osec, opk, ofp) = oplog_keypair(1);
        let (msec, mpk, mfp) = oplog_keypair(2);
        let (rsec, rpk, _rfp) = oplog_keypair(3);
        let (s2sec, s2pk, s2fp) = oplog_keypair(4);
        let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        let g = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &ofp,
            Some(Role::Owner),
            true,
            &ofp,
            1000,
            None,
            0,
        );
        let gs = g.sign(&osec);
        coll.append_signed_op(&g, &gs, &opk);
        let a = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &mfp,
            Some(Role::Editor),
            false,
            &ofp,
            1001,
            None,
            0,
        );
        let as_ = a.sign(&osec);
        coll.append_signed_op(&a, &as_, &opk);
        coll.author_register_recovery_key("KB", &mfp, &rpk, &msec, &mpk, 1002);
        // Owner removes m.
        let rm = coll.build_membership_op(
            "KB",
            MembershipAction::Remove,
            &mfp,
            None,
            false,
            &ofp,
            1003,
            None,
            0,
        );
        let rmsig = rm.sign(&osec);
        coll.append_signed_op(&rm, &rmsig, &opk);
        // m tries to recover via R → rejected (not a current member).
        coll.author_recovery_rebind(
            "KB",
            &mfp,
            &s2fp,
            &s2pk,
            &wrap_public_for(&s2sec),
            &rsec,
            &rpk,
            1004,
        );
        let m = derive_valid_members(&coll.oplog_ops(), &opk, 2000);
        assert!(
            !m.contains_key(&s2fp),
            "a removed member cannot recover its seat via the recovery key"
        );
        assert!(
            !m.contains_key(&mfp),
            "and the removed member stays removed"
        );
    }

    /// ADR-040 §Recovery-key (owner recovery): the OWNER recovers via its recovery key; the
    /// owner chain + governance/encryption readers honor the recovery-signed Rebind, so the
    /// successor is resolved as Owner and the KB stays E2e.
    #[test]
    fn owner_recovery_via_recovery_key_preserves_owner_chain() {
        use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
        use crate::membership::{derive_encryption, derive_valid_members};
        let (osec, opk, ofp) = oplog_keypair(1);
        let (rsec, rpk, _rfp) = oplog_keypair(3); // owner's offline recovery key
        let (o2sec, o2pk, o2fp) = oplog_keypair(2); // recovered owner identity
        let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        let k = ContentKey::generate();
        coll.author_e2e_genesis(
            "KB",
            &ofp,
            &osec,
            &opk,
            wrap_to_member(&k, &wrap_public_for(&osec)).unwrap(),
            1000,
        );
        coll.author_register_recovery_key("KB", &ofp, &rpk, &osec, &opk, 1001);
        // Owner primary LOST → recover owner → owner2 via R.
        coll.author_recovery_rebind(
            "KB",
            &ofp,
            &o2fp,
            &o2pk,
            &wrap_public_for(&o2sec),
            &rsec,
            &rpk,
            1002,
        );
        let peer = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
        let m = derive_valid_members(&peer.oplog_ops(), &opk, 2000);
        assert_eq!(
            m.get(&o2fp).map(|x| x.role),
            Some(Role::Owner),
            "recovered owner identity is Owner"
        );
        assert!(!m.contains_key(&ofp), "the lost owner key is retired");
        assert_eq!(
            derive_encryption(&peer.oplog_ops(), &opk),
            Encryption::E2e,
            "the KB stays E2e across owner recovery (owner chain honors the recovery Rebind)"
        );
    }

    /// ADR-040 PR2b (owner self-rotation): the OWNER rotates their own identity on an E2e KB
    /// they own. The Rebind is signed by the OLD owner key (still valid at the rebind's causal
    /// point); the content-key re-wrap to the NEW owner key must be signed by the NEW key (the
    /// old is retired the instant the Rebind lands) while derivation still anchors on the
    /// ORIGINAL genesis owner pubkey — the `anchor != signer` case. Proven on a fresh peer
    /// replica: owner2 is Owner, the predecessor owner is retired, owner2 decrypts, and the
    /// KB is still latched E2e via the owner chain.
    #[test]
    fn owner_self_rotation_rewraps_to_new_key_anchored_on_genesis() {
        use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
        use crate::membership::{derive_content_key, derive_encryption, derive_valid_members};
        let (osec, opk, ofp) = oplog_keypair(1); // genesis owner (the anchor, forever)
        let (o2sec, o2pk, o2fp) = oplog_keypair(2); // owner's NEW identity
        let (_xsec, _xpk, xfp) = oplog_keypair(3); // a non-member
        let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        let k = ContentKey::generate();
        // Owner enables e2e (genesis self-wrap to the owner's OLD wrap key).
        let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
        coll.author_e2e_genesis("KB", &ofp, &osec, &opk, self_wrap, 1000);

        // Owner rotates: the OLD owner key signs Rebind(owner → owner2).
        coll.author_rebind(
            "KB",
            &ofp,
            &o2fp,
            &o2pk,
            &wrap_public_for(&o2sec),
            &osec,
            &opk,
            1001,
        );
        // Owner re-wraps the content key to the NEW owner key — signed by owner2, anchored on
        // the OLD genesis pubkey (anchor != signer).
        let new_wrap = wrap_to_member(&k, &wrap_public_for(&o2sec)).unwrap();
        coll.author_rebind_rewrap(
            "KB", &o2fp, &o2pk, new_wrap, /*anchor*/ &opk, /*signer*/ &o2fp, &o2sec,
            &o2pk, 1002,
        );

        // Fresh peer replica: derive everything from the anchored (OLD) genesis pubkey.
        let peer = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
        let m = derive_valid_members(&peer.oplog_ops(), &opk, 2000);
        assert!(!m.contains_key(&ofp), "predecessor owner retired");
        assert_eq!(
            m.get(&o2fp).map(|x| x.role),
            Some(Role::Owner),
            "successor inherits Owner"
        );
        assert_eq!(
            derive_encryption(&peer.oplog_ops(), &opk),
            Encryption::E2e,
            "still e2e via the owner chain after rotation"
        );
        assert_eq!(
            derive_content_key(&peer.oplog_ops(), &opk, &o2fp, &o2sec).map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "rotated owner recovers the content key with the new key"
        );
        assert!(
            derive_content_key(&peer.oplog_ops(), &opk, &xfp, &_xsec).is_none(),
            "a non-member recovers nothing"
        );
    }

    // Regression for the join-decrypt bug (branch `fix/joiner-content-sync`): the OWNER must
    // author the member `Admit` against the CURRENT collection lineage (the network task's fresh
    // replica that already holds the genesis + SetEncryption it authored at enable) — NOT a STALE
    // pre-enable snapshot. Authoring against a stale base (which has no oplog map) re-creates the
    // oplog `MapPrelim` root; when the key-blind daemon merges that delta it TOMBSTONES the live
    // genesis/SetEncryption ops, the admit becomes a phantom second-genesis (empty `prev_hash`),
    // and the joiner gets a corrupt op-log it can't derive a key from. This pins the FIX (chain
    // cleanly, converge, member derives) and the precise root-cause property (the admit CHAINS
    // onto the SetEncryption head rather than masquerading as a genesis).
    #[test]
    fn member_admit_must_chain_on_the_current_collection_not_a_stale_pre_enable_base() {
        use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
        use crate::membership::{derive_content_key, derive_encryption};
        let (osec, opk, ofp) = oplog_keypair(11);
        let (msec, mpk, mfp) = oplog_keypair(22);

        // 1) Owner shares: owner set, NO membership op-log yet (mirror of the share path).
        let shared = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        let shared_state = shared.encode_state();
        // The key-blind daemon relays the owner-signed collection bytes verbatim.
        let mut daemon = KbCollectionDoc::from_bytes(&shared_state).unwrap();

        // 2) Owner ENABLES e2e against a reconstruction of the shared collection.
        let k = ContentKey::generate();
        let mut live = KbCollectionDoc::from_bytes(&shared_state).unwrap();
        let enable_delta = live.author_e2e_genesis(
            "KB",
            &ofp,
            &osec,
            &opk,
            wrap_to_member(&k, &wrap_public_for(&osec)).unwrap(),
            1000,
        );
        daemon.apply_update(&enable_delta).unwrap();
        assert_eq!(
            daemon.oplog_ops().len(),
            2,
            "daemon holds genesis + SetEncryption after enable"
        );
        let head_before_admit = {
            // The op the admit MUST chain onto (the SetEncryption, latest in causal order).
            let ops = daemon.oplog_ops();
            ops.iter()
                .find(|o| o.op.action == MembershipAction::SetEncryption)
                .map(|o| o.chain_hash())
                .expect("SetEncryption present")
        };

        // 3a) ROOT-CAUSE GUARD — authoring against the STALE pre-enable base produces a phantom
        //     genesis (empty prev_hash): it has NO knowledge of the genesis/SetEncryption head.
        {
            let mut stale = KbCollectionDoc::from_bytes(&shared_state).unwrap(); // pre-enable!
            stale.author_member_admit(
                "KB",
                &mfp,
                &mpk,
                &wrap_public_for(&msec),
                Role::Editor,
                "m",
                wrap_to_member(&k, &wrap_public_for(&msec)).unwrap(),
                &ofp,
                &osec,
                &opk,
                1001,
            );
            let admit = stale
                .oplog_ops()
                .into_iter()
                .find(|o| o.op.subject == mfp)
                .expect("admit authored");
            assert!(
                admit.op.prev_hash.is_empty(),
                "stale-base admit masquerades as a genesis (the bug) — empty prev_hash"
            );
        }

        // 3b) THE FIX — author against the CURRENT collection (the daemon's lineage). The admit
        //     CHAINS onto the SetEncryption head; merging it is purely additive (no tombstone).
        let mut current = KbCollectionDoc::from_bytes(&daemon.encode_state()).unwrap();
        let good_delta = current.author_member_admit(
            "KB",
            &mfp,
            &mpk,
            &wrap_public_for(&msec),
            Role::Editor,
            "m",
            wrap_to_member(&k, &wrap_public_for(&msec)).unwrap(),
            &ofp,
            &osec,
            &opk,
            1001,
        );
        let admit = current
            .oplog_ops()
            .into_iter()
            .find(|o| o.op.subject == mfp)
            .expect("admit authored");
        assert_eq!(
            admit.op.prev_hash, head_before_admit,
            "current-base admit chains onto the SetEncryption head (the fix)"
        );

        daemon.apply_update(&good_delta).unwrap();
        // Adversarial: a duplicate echo of our own op must be idempotent (the racy re-apply path).
        daemon.apply_update(&good_delta).unwrap();
        assert_eq!(
            daemon.oplog_ops().len(),
            3,
            "genesis + SetEncryption + admit all survive the merge (and the echo)"
        );

        // The joiner, with ONLY the daemon-relayed collection, derives the SAME content key AND
        // sees E2e mode intact — the user-visible success the bug denied.
        let joiner = KbCollectionDoc::from_bytes(&daemon.encode_state()).unwrap();
        assert_eq!(
            derive_encryption(&joiner.oplog_ops(), &opk),
            Encryption::E2e,
            "E2e mode survives (genesis anchor + SetEncryption intact)"
        );
        assert_eq!(
            derive_content_key(&joiner.oplog_ops(), &opk, &mfp, &msec).map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "the approved member derives the content key from the relayed collection"
        );
        // A non-member still derives nothing.
        let (xsec, _xpk, xfp) = oplog_keypair(33);
        assert!(
            derive_content_key(&joiner.oplog_ops(), &opk, &xfp, &xsec).is_none(),
            "a non-member recovers no key"
        );
    }

    #[test]
    fn rotate_on_remove_rekeys_remaining_members_and_strands_the_removed_one() {
        // ADR-037 §D3 — the SELECTIVE security oracle. 3 members (owner + B + C) share key k.
        // Remove B with a fresh k'. The remaining two must CONVERGE on k' and the removed B
        // must keep ONLY the old k (reads nothing new), not break entirely — proving the
        // rotation denies k' specifically, rather than just severing B's pipeline.
        use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
        use crate::membership::{
            derive_content_key, derive_governance, derive_valid_members_governed, MembershipView,
        };
        let (osec, opk, ofp) = oplog_keypair(1);
        let (bsec, bpk, bfp) = oplog_keypair(2);
        let (csec, cpk, cfp) = oplog_keypair(3);

        let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        let k = ContentKey::generate();
        coll.author_e2e_genesis(
            "KB",
            &ofp,
            &osec,
            &opk,
            wrap_to_member(&k, &wrap_public_for(&osec)).unwrap(),
            1000,
        );
        coll.author_member_admit(
            "KB",
            &bfp,
            &bpk,
            &wrap_public_for(&bsec),
            Role::Editor,
            "b",
            wrap_to_member(&k, &wrap_public_for(&bsec)).unwrap(),
            &ofp,
            &osec,
            &opk,
            1001,
        );
        coll.author_member_admit(
            "KB",
            &cfp,
            &cpk,
            &wrap_public_for(&csec),
            Role::Editor,
            "c",
            wrap_to_member(&k, &wrap_public_for(&csec)).unwrap(),
            &ofp,
            &osec,
            &opk,
            1002,
        );
        // Everyone holds k before rotation.
        for (fp, sec) in [(&ofp, &osec), (&bfp, &bsec), (&cfp, &csec)] {
            assert_eq!(
                derive_content_key(&coll.oplog_ops(), &opk, fp, sec).map(|c| *c.as_bytes()),
                Some(*k.as_bytes()),
                "every member holds k before rotation"
            );
        }
        let c_epoch_before = coll.epoch_of(&cfp);
        // Exact pre-rotation replica (matching chain hashes) so the rotation DELTA grafts.
        let pre_rotation_state = coll.encode_state();

        // Rotate: remove B, re-wrap a FRESH k' to the remaining members (owner + C).
        let k2 = ContentKey::generate();
        assert_ne!(k.as_bytes(), k2.as_bytes(), "fresh rotation key");
        let rewraps = vec![
            (
                ofp.clone(),
                wrap_to_member(&k2, &wrap_public_for(&osec)).unwrap(),
            ),
            (
                cfp.clone(),
                wrap_to_member(&k2, &wrap_public_for(&csec)).unwrap(),
            ),
        ];
        let delta = coll.author_rotate_on_remove("KB", &bfp, &rewraps, &ofp, &osec, &opk, 2000);

        let ops = coll.oplog_ops();
        // (1) The two remaining members converge on k'.
        assert_eq!(
            derive_content_key(&ops, &opk, &ofp, &osec).map(|c| *c.as_bytes()),
            Some(*k2.as_bytes()),
            "owner re-keys to k'"
        );
        assert_eq!(
            derive_content_key(&ops, &opk, &cfp, &csec).map(|c| *c.as_bytes()),
            Some(*k2.as_bytes()),
            "remaining member C re-keys to k'"
        );
        // (2) THE ORACLE: the removed B still derives the OLD k (its last wrap) — NOT k', and
        // NOT nothing. It can decrypt pre-rotation content but no post-rotation ciphertext.
        assert_eq!(
            derive_content_key(&ops, &opk, &bfp, &bsec).map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "removed B keeps ONLY the old k — stranded from k'"
        );
        // (3) B is gone from derived membership; owner + C remain with UNCHANGED attributes
        // (the re-key Admit must not silently downgrade role/can_invite or bump epoch).
        let gov = derive_governance(&ops, &opk);
        let members =
            derive_valid_members_governed(&ops, &opk, 2000, gov, &MembershipView::default());
        assert!(!members.contains_key(&bfp), "B removed from membership");
        assert_eq!(members.len(), 2, "only owner + C remain");
        assert_eq!(members[&ofp].role, Role::Owner, "owner role preserved");
        assert!(
            members[&ofp].can_invite,
            "owner can_invite preserved (genesis)"
        );
        assert_eq!(members[&cfp].role, Role::Editor, "C role preserved");
        assert_eq!(
            members[&cfp].epoch, c_epoch_before,
            "C epoch NOT bumped by re-key"
        );

        // (4) Convergence: a replica at the pre-rotation state applies ONLY the relayed delta
        // (as the key-blind daemon ships it) and agrees on every point.
        let mut peer = KbCollectionDoc::from_bytes(&pre_rotation_state).unwrap();
        peer.apply_update(&delta).unwrap();
        let pops = peer.oplog_ops();
        assert_eq!(
            derive_content_key(&pops, &opk, &cfp, &csec).map(|c| *c.as_bytes()),
            Some(*k2.as_bytes()),
            "peer: C converges on k'"
        );
        assert_eq!(
            derive_content_key(&pops, &opk, &bfp, &bsec).map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "peer: removed B still stranded on old k"
        );
        assert!(
            !derive_valid_members_governed(
                &pops,
                &opk,
                2000,
                derive_governance(&pops, &opk),
                &MembershipView::default()
            )
            .contains_key(&bfp),
            "peer agrees B is removed"
        );

        // (5) The Remove op is genuinely owner-signed (not a daemon-forged membership change).
        let remove = ops
            .iter()
            .find(|o| o.op.action == MembershipAction::Remove && o.op.subject == bfp)
            .expect("a Remove op for B exists");
        assert!(
            remove.verify_signed(),
            "Remove is a verifiable owner signature"
        );
        assert_eq!(
            remove.author_pubkey, opk,
            "Remove authored by the owner key"
        );
    }

    /// ADR-037 #167/#168 — on an E2e KB a deletion is SEALED into a client-id-stamped
    /// outer op-set op, so `update_new_op_authors` attributes it to the SEAL client_id and
    /// the ADR-023 fence rejects a stale-epoch sealed delete. This is WHY #168's always-seal
    /// closes #167's deletion-fence gap for E2e KBs. (Contrast: a PLAINTEXT pure-delete is
    /// unattributable — yrs tombstones carry no deleter — the residual #167 gap on the
    /// unencrypted path, which needs a separate deleter-attribution design.)
    #[test]
    fn update_new_op_authors_attributes_a_sealed_delete_to_the_seal_client() {
        use crate::content_crypto::ContentKey;
        use crate::op_set;
        let key = ContentKey::generate();
        let mut node = KbNodeDoc::new_with_client_id("n", "T", "secret-body", &[], 1);
        let create = node.encode_state();
        let inner_delete = node.set_body(""); // a pure delete at the plaintext layer
                                              // Owner seals the create (op-set base); the attacker seals the delete at a STALE epoch.
        let valid_cid = derive_kb_client_id("SHA256:owner", 0);
        let (_i0, outer0) = op_set::seal_op(&[], &key, &create, valid_cid).unwrap();
        let base = op_set::merge(&[], &outer0).unwrap();
        let stale_cid = derive_kb_client_id("SHA256:attacker", 0);
        let (_i1, outer1) = op_set::seal_op(&base, &key, &inner_delete, stale_cid).unwrap();
        // The fence's author extraction reports the STALE seal client (so it rejects it),
        // even though the INNER op is a pure (otherwise-unattributable) delete.
        let authors = update_new_op_authors(&outer1, &base).unwrap();
        assert!(
            authors.contains(&stale_cid),
            "the sealed delete's outer op carries the stale seal client_id ⇒ the fence catches it"
        );
        assert!(
            !authors.contains(&valid_cid),
            "the prior op-set base is grandfathered, not re-reported as a new author"
        );
    }

    #[test]
    fn oplog_head_advances_along_the_chain() {
        let (osec, opub, owner_fp) = oplog_keypair(1);
        let (_bsec, _bpub, bob_fp) = oplog_keypair(2);
        let mut coll = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");

        let g = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &owner_fp,
            Some(Role::Owner),
            true,
            &owner_fp,
            1,
            None,
            0,
        );
        let gsig = g.sign(&osec);
        coll.append_signed_op(&g, &gsig, &opub);
        let ghash = g.chain_hash(&gsig);
        assert_eq!(coll.oplog_head(), Some(ghash.clone()));

        // Owner admits bob; the new op chains off the genesis head.
        let a = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &bob_fp,
            Some(Role::Editor),
            false,
            &owner_fp,
            2,
            None,
            0,
        );
        assert_eq!(a.prev_hash, ghash, "second op chains off genesis");
        let asig = a.sign(&osec);
        coll.append_signed_op(&a, &asig, &opub);
        assert_eq!(coll.oplog_len(), 2);
        assert_eq!(
            coll.oplog_head(),
            Some(a.chain_hash(&asig)),
            "head advanced to the admit"
        );
    }

    #[test]
    fn oplog_concurrent_appends_converge_as_a_set() {
        let (osec, opub, owner_fp) = oplog_keypair(1);
        let (_sx, _px, x_fp) = oplog_keypair(2);
        let (_sy, _py, y_fp) = oplog_keypair(3);

        let mut base = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");
        let g = base.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &owner_fp,
            Some(Role::Owner),
            true,
            &owner_fp,
            1,
            None,
            0,
        );
        let gsig = g.sign(&osec);
        base.append_signed_op(&g, &gsig, &opub);
        let state = base.encode_state();

        // Two replicas concurrently admit DIFFERENT subjects, both off genesis.
        let mut a = KbCollectionDoc::from_bytes(&state).unwrap();
        let mut b = KbCollectionDoc::from_bytes(&state).unwrap();
        let opx = a.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &x_fp,
            Some(Role::Editor),
            false,
            &owner_fp,
            2,
            None,
            0,
        );
        let sx = opx.sign(&osec);
        let ux = a.append_signed_op(&opx, &sx, &opub);
        let opy = b.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &y_fp,
            Some(Role::Editor),
            false,
            &owner_fp,
            3,
            None,
            0,
        );
        let sy = opy.sign(&osec);
        let uy = b.append_signed_op(&opy, &sy, &opub);

        // Cross-apply the concurrent updates.
        a.apply_update(&uy).unwrap();
        b.apply_update(&ux).unwrap();

        // Both replicas hold all three ops (set union; no lost append).
        assert_eq!(a.oplog_len(), 3);
        assert_eq!(b.oplog_len(), 3);
        // The deterministic head pick (highest-hash tip) agrees on both peers.
        assert_eq!(a.oplog_head(), b.oplog_head());
    }

    #[test]
    fn oplog_record_with_mismatched_pubkey_fails_verify() {
        let (osec, _opub, owner_fp) = oplog_keypair(1);
        let (_msec, mpub, _mfp) = oplog_keypair(9);
        let mut coll = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");

        // The op names + is signed by the owner, but the record stores a DIFFERENT
        // author_pubkey (a relay swapping the key). Decode succeeds; verify fails.
        let op = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &owner_fp,
            Some(Role::Owner),
            true,
            &owner_fp,
            1,
            None,
            0,
        );
        let sig = op.sign(&osec);
        coll.append_signed_op(&op, &sig, &mpub); // wrong pubkey stored

        let ops = coll.oplog_ops();
        assert_eq!(ops.len(), 1);
        assert!(
            !ops[0].verify_signed(),
            "fingerprint(author_pubkey) != author ⇒ record rejected"
        );
    }
}
