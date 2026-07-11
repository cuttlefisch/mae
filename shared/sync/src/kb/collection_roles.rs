//! `KbCollectionDoc`: ownership & roles (ADR-018 v2 schema) — owner,
//! per-principal roles, join policy, transport policy, encryption mode, and
//! pending join requests. Includes the ADR-023 epoch-fenced-rebase private
//! helpers (promoted `pub(super)` where a sibling module needs them).

use yrs::{Array, ArrayPrelim, Map, MapPrelim, MapRef, Out, ReadTxn, Transact};

use super::*;
use crate::text::{new_doc, new_doc_with_client_id};

impl KbCollectionDoc {
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
    ///
    /// `pub(super)`: also called from `collection_crypto::migrate_if_legacy`.
    pub(super) fn member_roles_map(root: &MapRef, txn: &mut yrs::TransactionMut) -> MapRef {
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
}
