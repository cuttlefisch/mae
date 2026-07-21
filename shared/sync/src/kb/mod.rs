//! KB CRDT documents: `KbNodeDoc` (single node) and `KbCollectionDoc` (manifest
//! of nodes, membership, roles, and the signed membership op-log).
//!
//! Split by clean per-type / per-theme seams: [`node`] holds `KbNodeDoc`
//! (zero coupling to the collection type); `collection_core` /
//! `collection_roles` / `collection_oplog` / `collection_crypto` hold
//! `KbCollectionDoc`'s impl, split by CRUD / ownership+roles / signed oplog /
//! E2E crypto authoring. A handful of private cross-cutting helpers are
//! `pub(super)` here so the split files can share them.

use yrs::updates::decoder::Decode;
use yrs::Doc;

use crate::SyncError;

mod collection_core;
mod collection_crypto;
mod collection_oplog;
mod collection_roles;
mod node;

pub use node::{KbNodeDoc, MaterializedNode};

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
    crate::text::fold_hash_to_yrs_client_id(h)
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

#[cfg(test)]
mod tests;
