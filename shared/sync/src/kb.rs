//! KbNodeDoc: yrs-backed KB node with YMap schema.
//!
//! All yrs Doc instances use UTF-16 offset kind (via `text::new_doc()`) for
//! consistency with the Yjs standard. See the CRDT UTF-16 fix (92a20b8).

use sha2::{Digest, Sha256};
use yrs::{
    updates::decoder::Decode, updates::encoder::Encode, Array, ArrayPrelim, Doc, GetString, Map,
    MapPrelim, MapRef, Out, ReadTxn, Text, TextPrelim, Transact,
};

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
const MEMBER_ROLE_KEY: &str = "role";
const MEMBER_LABEL_KEY: &str = "label";
/// ADR-023: per-member monotonic authorization epoch, bumped by the daemon on
/// every role change for that member. Drives the epoch-fenced rebase: the KB
/// client_id a member authors under is `derive_kb_client_id(fp, epoch)`, so a
/// role change rotates it and the daemon can fence pre-grant (stale-epoch) ops.
/// Stored as a decimal string (mirrors the role/label string fields).
const MEMBER_EPOCH_KEY: &str = "epoch";
const PENDING_AT_KEY: &str = "requested_at";

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
                    out.push(PendingRequest {
                        fingerprint: fp.to_string(),
                        label,
                        requested_at,
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

    /// Record a pending join request (idempotent re-request).
    pub fn add_pending(&mut self, principal: &str, label: &str, requested_at: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        let p = match root.get(&txn, COLL_PENDING_KEY) {
            Some(Out::YMap(p)) => p,
            _ => root.insert(&mut txn, COLL_PENDING_KEY, MapPrelim::default()),
        };
        let req = p.insert(&mut txn, principal, MapPrelim::default());
        req.insert(&mut txn, MEMBER_LABEL_KEY, label);
        req.insert(&mut txn, PENDING_AT_KEY, requested_at);
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
        coll.add_pending("SHA256:bob", "bob", "2026-06-16T00:00:00Z");
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
        coll.add_pending("SHA256:eve", "eve", "t");
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
}
