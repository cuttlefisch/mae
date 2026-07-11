//! `KbCollectionDoc`: the ADR-026 signed membership op-log — the append-only,
//! CRDT *set* of signed membership ops (`YMap<chain_hash -> op record>`).
//! Validity is *derived* by every peer replaying this log, never read as a
//! trusted verdict.

use yrs::updates::decoder::Decode;
use yrs::{Map, MapPrelim, MapRef, Out, ReadTxn, Transact};

use super::*;
use crate::membership::{MembershipAction, MembershipOp, SignedMembershipOp};

impl KbCollectionDoc {
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
    // PERF/DOGFOOD(#247): the membership op-log is APPEND-ONLY and never pruned — it grows for the
    // KB's lifetime (one op per admit/remove/role-change/rebind/recovery-key-register). Every
    // derive decodes the whole log; a long-lived, high-churn shared KB will accumulate thousands
    // of ops. Op-log checkpointing/compaction is a v0.16 item — the dogfood measures where this
    // actually walls before we build it. See ADR-042.
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
}
