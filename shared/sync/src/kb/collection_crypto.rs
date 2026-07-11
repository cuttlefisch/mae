//! `KbCollectionDoc`: ADR-037/038/040/041 E2E crypto authoring (genesis,
//! member admit, rotate-on-remove, rebind, recovery-key registration +
//! recovery rebind) plus member pubkey/wrap-pubkey storage and the v1->v2
//! legacy-schema migration.

use yrs::{Array, Map, MapPrelim, Out, ReadTxn, Transact};

use super::*;
use crate::membership::MembershipAction;

impl KbCollectionDoc {
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
