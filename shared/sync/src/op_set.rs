//! ADR-037 op-set sync model — the pure layer (#146 Phase 1).
//!
//! For an `Encryption::E2e` KB, a node's doc (`kb:{node_id}`) is NOT the plaintext
//! [`crate::kb::KbNodeDoc`] but a yrs **op-set**: a `YMap<op_id → encrypted_blob>`.
//! Each editor edit produces a plaintext incremental yrs update; the editor seals it
//! ([`seal_op`]) — encrypt under the per-KB content key, then build the **outer** yrs
//! update that inserts the ciphertext blob into the op-set, stamped with the editor's
//! epoch-aware KB client id. The daemon sees a normal yrs doc: its existing
//! sign-verify / epoch-fence / merge / state-vector reconcile all work **key-blind**
//! (the blob is opaque — the daemon never holds the content key). A member
//! materializes the text by opening the new blobs ([`open_new_ops`]) and applying the
//! inner plaintext updates to its local `KbNodeDoc` in **causal order**. (The op-set
//! is an unordered *set*, but a causally-dependent edit applied before its predecessor
//! does NOT converge to the same yrs state — so [`open_new_ops`] returns ops in causal
//! order, recovered from the yrs clocks already inside each update; no extra ordering
//! metadata is stored.)
//!
//! This module is the pure substrate (seal / merge / open + `op_id` hashing), reusing
//! [`crate::content_crypto`]. The editor/daemon wiring + the live confidentiality
//! oracle are later phases. The unencrypted path is untouched.

use crate::content_crypto::{decrypt, encrypt, ContentKey};
use crate::text::new_doc_with_client_id;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use yrs::{updates::decoder::Decode, Map, ReadTxn, StateVector, Transact, Update};

/// The op-set's YMap name within its yrs doc.
const OPS_MAP: &str = "ops";

/// A failure manipulating the op-set yrs doc (a malformed update). Decrypt failures
/// are NOT errors here — [`open_new_ops`] skips an op that doesn't open (wrong key /
/// tamper), so a non-member simply materializes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpSetError {
    Yrs(String),
}

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// The op id for a ciphertext blob: `hex(sha256(blob))` — stable, and distinct per
/// ciphertext (a fresh AEAD nonce per encrypt ⇒ even the same plaintext yields a new
/// op id, so re-sealing is a new op, never a silent overwrite).
pub fn op_id_of(blob: &[u8]) -> String {
    hex::encode(Sha256::digest(blob))
}

/// Seal a plaintext node update into an encrypted op-set entry (ADR-037 §D1). Encrypts
/// `plaintext_update` under `content_key`, then builds the INCREMENTAL yrs update that
/// inserts `ops[op_id] = base64(blob)`, stamped with `client_id` (the editor's
/// epoch-aware KB client id — so the daemon's ADR-023 fence authorizes the right
/// author on the outer op). `op_set_state` is the node's current op-set yrs state
/// (empty for a fresh node), loaded so the client's clock continues monotonically.
/// Returns `(op_id, outer_update_bytes)` — push + locally [`merge`] the update.
pub fn seal_op(
    op_set_state: &[u8],
    content_key: &ContentKey,
    plaintext_update: &[u8],
    client_id: u64,
) -> Result<(String, Vec<u8>), OpSetError> {
    let blob = encrypt(content_key, plaintext_update);
    let op_id = op_id_of(&blob);
    let doc = new_doc_with_client_id(client_id);
    if !op_set_state.is_empty() {
        let upd = Update::decode_v1(op_set_state).map_err(|e| OpSetError::Yrs(e.to_string()))?;
        let mut txn = doc.transact_mut();
        txn.apply_update(upd)
            .map_err(|e| OpSetError::Yrs(e.to_string()))?;
    }
    // Everything new from here (incl. the map's creation on a fresh node) is the diff.
    let before = doc.transact().state_vector();
    let map = doc.get_or_insert_map(OPS_MAP);
    {
        let mut txn = doc.transact_mut();
        map.insert(&mut txn, op_id.as_str(), b64().encode(&blob));
    }
    let diff = doc.transact().encode_state_as_update_v1(&before);
    Ok((op_id, diff))
}

/// Merge an op-set update (a remote peer's, or our own [`seal_op`] output) into an
/// op-set state, returning the new full state. yrs set-union: concurrent inserts of
/// distinct `op_id`s converge; a re-applied op is idempotent.
pub fn merge(op_set_state: &[u8], update: &[u8]) -> Result<Vec<u8>, OpSetError> {
    let doc = new_doc_with_client_id(1);
    {
        let mut txn = doc.transact_mut();
        if !op_set_state.is_empty() {
            let s = Update::decode_v1(op_set_state).map_err(|e| OpSetError::Yrs(e.to_string()))?;
            txn.apply_update(s)
                .map_err(|e| OpSetError::Yrs(e.to_string()))?;
        }
        let u = Update::decode_v1(update).map_err(|e| OpSetError::Yrs(e.to_string()))?;
        txn.apply_update(u)
            .map_err(|e| OpSetError::Yrs(e.to_string()))?;
    }
    let full = doc
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    Ok(full)
}

/// The set of `op_id`s present in an op-set state.
pub fn op_ids(op_set_state: &[u8]) -> BTreeSet<String> {
    let doc = new_doc_with_client_id(1);
    if !op_set_state.is_empty() {
        if let Ok(upd) = Update::decode_v1(op_set_state) {
            let mut txn = doc.transact_mut();
            let _ = txn.apply_update(upd);
        }
    }
    let map = doc.get_or_insert_map(OPS_MAP);
    let txn = doc.transact();
    map.iter(&txn).map(|(k, _)| k.to_string()).collect()
}

/// Open (decrypt) the op-set entries whose `op_id` is NOT in `seen`, returned in
/// **causal order** — the editor applies each inner plaintext update to its local
/// `KbNodeDoc` in this order. The op-set is an unordered *set* of blobs; the apply
/// order matters, because yrs does NOT converge a causally-dependent edit applied
/// before its predecessor to the same result (e.g. two successive title edits).
/// Causal order is recovered with no extra metadata, from the yrs state-vector inside
/// each decrypted update: an op that builds on more history covers a larger clock
/// total, so ascending clock-total puts the node-creation first and each edit after
/// the ops it depends on. (`op_id` breaks ties deterministically.) A blob that does
/// NOT open (wrong key / tampered ciphertext / corrupt base64) is **skipped** — a
/// non-member / relay materializes nothing — but IS counted in the returned
/// `undecryptable` total (#206), so a caller can distinguish "wrong/rotated key" from
/// "not yet received" instead of both looking identically like silence.
pub struct OpenedOps {
    /// Decrypted ops, in causal order.
    pub ops: Vec<(String, Vec<u8>)>,
    /// Count of present-but-undecryptable op-set entries (wrong/rotated key, tamper,
    /// or corrupt encoding) among the entries NOT in `seen`.
    pub undecryptable: usize,
}

pub fn open_new_ops(
    op_set_state: &[u8],
    content_key: &ContentKey,
    seen: &BTreeSet<String>,
) -> OpenedOps {
    let doc = new_doc_with_client_id(1);
    if !op_set_state.is_empty() {
        if let Ok(upd) = Update::decode_v1(op_set_state) {
            let mut txn = doc.transact_mut();
            let _ = txn.apply_update(upd);
        }
    }
    let map = doc.get_or_insert_map(OPS_MAP);
    let txn = doc.transact();
    let mut out: Vec<(String, Vec<u8>, u64)> = Vec::new();
    let mut undecryptable = 0usize;
    for (op_id, val) in map.iter(&txn) {
        if seen.contains(op_id) {
            continue;
        }
        let Ok(blob) = b64().decode(val.to_string(&txn)) else {
            undecryptable += 1;
            continue;
        };
        match decrypt(content_key, &blob) {
            Ok(plaintext) => {
                // Causal rank = total of the update's covered clocks (history depth).
                let rank = Update::decode_v1(&plaintext)
                    .map(|u| u.state_vector().iter().map(|(_, &c)| c as u64).sum())
                    .unwrap_or(0);
                out.push((op_id.to_string(), plaintext, rank));
            }
            Err(_) => undecryptable += 1,
        }
    }
    out.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
    OpenedOps {
        ops: out.into_iter().map(|(id, pt, _)| (id, pt)).collect(),
        undecryptable,
    }
}

/// Materialize an op-set into a plaintext [`crate::kb::KbNodeDoc`]: open every op (in
/// causal order, via [`open_new_ops`]) and apply the inner plaintext updates to ONE
/// node doc, built FROM the ops (so there is no pre-created structure to conflict
/// with). A non-member (empty open, or the wrong key) gets an empty node — this is the
/// reference "member-side lazy-fetch decrypt" primitive (ADR-053/Phase G, #382): the
/// daemon never calls this (it never holds `key`); a thin client does, after fetching
/// `kb/query.get`'s raw `ciphertext_b64` for an `Encryption::E2e` KB.
pub fn materialize(op_set_state: &[u8], key: &ContentKey) -> crate::kb::KbNodeDoc {
    let opened = open_new_ops(op_set_state, key, &BTreeSet::new());
    if opened.ops.is_empty() {
        return crate::kb::KbNodeDoc::new_with_client_id("n1", "", "", &[], 99);
    }
    let mut node = crate::kb::KbNodeDoc::from_bytes(&opened.ops[0].1).unwrap();
    for (_id, plaintext) in &opened.ops[1..] {
        node.apply_update(plaintext).unwrap();
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kb::KbNodeDoc;

    /// Drive an authoring KbNodeDoc through a few edits, sealing each update into a
    /// growing op-set. Op 0 is the node's initial STRUCTURE (`encode_state` — the
    /// yrs object ids a reader needs); the rest are incremental edits carrying the
    /// secret content. Returns (op_set_state, the author's final title, body).
    fn author_session(key: &ContentKey, client_id: u64) -> (Vec<u8>, String, String) {
        let mut node = KbNodeDoc::new_with_client_id("n1", "", "", &[], client_id);
        let mut state: Vec<u8> = Vec::new();
        let plaintexts = vec![
            node.encode_state(), // op 0: structure
            node.set_title("Secret Title"),
            node.set_body("private body text"),
            node.set_title("Secret Title v2"),
        ];
        for pt in &plaintexts {
            let (_op_id, outer) = seal_op(&state, key, pt, client_id).unwrap();
            state = merge(&state, &outer).unwrap();
        }
        (state, node.title(), node.body())
    }

    #[test]
    fn seal_open_round_trips_and_converges_out_of_order() {
        // Loop with fresh keys/nonces: `op_id` = hash(ciphertext) varies every run, so
        // `open_new_ops` returns ops in a different order each iteration — this is what
        // surfaces an order-dependence bug (a single run can pass by luck). Both apply
        // orders must converge to the author's exact title + body.
        for _ in 0..50 {
            let key = ContentKey::generate();
            let (state, title, body) = author_session(&key, 7);
            let m = materialize(&state, &key);
            assert_eq!(m.title(), title, "title converges from any storage order");
            assert_eq!(m.body(), body, "body converges from any storage order");
        }
    }

    #[test]
    fn reseal_on_enable_grafts_onto_a_plaintext_shared_node_then_edits_chain() {
        // #171 re-encryption-on-enable: a node SHARED PLAINTEXT before E2e is enabled has a
        // plaintext daemon lineage under the owner's content client_id. The seal path is
        // pinned to that SAME client_id by the ADR-023 fence, so a naive first seal (fresh
        // op-set at clock 0) OVERLAPS the plaintext clocks and yrs drops it. The fix re-seals
        // op 0 with the op-set SEEDED by the plaintext state, so the OPS_MAP CONTINUES the
        // lineage (clock K+1) and grafts. This pins that property end-to-end.
        use crate::kb::{derive_kb_client_id, KbNodeDoc};
        let owner_cid = derive_kb_client_id("SHA256:owner-fp", 0);

        // 1) The plaintext node the daemon already holds (shared before enable).
        let mut node =
            KbNodeDoc::new_with_client_id("n1", "Original Title", "original body", &[], owner_cid);
        let plaintext_state = node.encode_state();
        let mut daemon = KbNodeDoc::from_bytes(&plaintext_state).unwrap();

        // 2) RE-SEAL ON ENABLE — seed `seal_op` with the plaintext state (same client_id).
        let key = ContentKey::generate();
        let (op0_id, reseal) =
            seal_op(&plaintext_state, &key, &plaintext_state, owner_cid).unwrap();
        daemon.apply_update(&reseal).unwrap();
        assert!(
            op_ids(&daemon.encode_state()).contains(&op0_id),
            "re-seal op 0 grafts onto the daemon's plaintext lineage (no clock collision)"
        );

        // 3) SEVERAL sealed edits after enable chain onto the re-sealed op-set. This is the
        //    ordering-robustness case: re-seal op 0 carries the WHOLE pre-enable history (a
        //    high clock-total), while each edit is a small delta — `open_new_ops` sorts by
        //    ascending clock-total, so op 0 must still materialize FIRST and each edit after
        //    the ops it depends on, or a later edit would apply against a missing base.
        let mut op_set_state = merge(&plaintext_state, &reseal).unwrap();
        for pt in [
            node.set_body("CANARY body"),
            node.set_title("Sealed Title v2"),
            node.set_body("CANARY body — revised"),
        ] {
            let (op_id, sealed) = seal_op(&op_set_state, &key, &pt, owner_cid).unwrap();
            daemon.apply_update(&sealed).unwrap();
            op_set_state = merge(&op_set_state, &sealed).unwrap();
            assert!(
                op_ids(&daemon.encode_state()).contains(&op_id),
                "each sealed edit chains onto the re-sealed op-set on the daemon"
            );
        }

        // 4) A JOINER pulling the daemon's full node state materializes ALL ops in causal
        //    order and converges to the author's latest title + body (the #171 success).
        let joined = materialize(&daemon.encode_state(), &key);
        assert_eq!(
            joined.body(),
            "CANARY body — revised",
            "joiner converges to the latest sealed body across re-seal + N edits"
        );
        assert_eq!(
            joined.title(),
            "Sealed Title v2",
            "joiner converges to the latest sealed title (op 0 base + edits, ordered)"
        );
        // A wrong key still opens nothing from the merged daemon state — but #206:
        // the failures must be COUNTED (a wrong/rotated key), not indistinguishable
        // from "no ops present yet".
        let wrong_key_open = open_new_ops(
            &daemon.encode_state(),
            &ContentKey::generate(),
            &BTreeSet::new(),
        );
        assert!(
            wrong_key_open.ops.is_empty(),
            "a non-member key opens no ops even after re-seal"
        );
        assert_eq!(
            wrong_key_open.undecryptable, 4,
            "#206: every present op (op0 reseal + 3 edits) fails to decrypt and is counted, \
             not silently dropped"
        );
    }

    #[test]
    fn a_non_member_or_wrong_key_materializes_nothing() {
        let key = ContentKey::generate();
        let (state, _t, _b) = author_session(&key, 7);
        // A non-member holds the op-set (ciphertext) but a different key.
        let wrong = ContentKey::generate();
        let wrong_key_open = open_new_ops(&state, &wrong, &BTreeSet::new());
        assert!(wrong_key_open.ops.is_empty(), "wrong key opens no ops");
        // #206 adversarial case: the whole point of this issue is that "wrong key"
        // and "nothing received yet" must NOT look identical. 4 ops were authored
        // (op 0 structure + 3 edits — see author_session); all 4 must be counted as
        // undecryptable, not silently swallowed into an empty, indistinguishable result.
        assert_eq!(
            wrong_key_open.undecryptable, 4,
            "a wrong key must surface a nonzero undecryptable count, distinguishing \
             'wrong/rotated key' from 'not yet received'"
        );
        let blind = materialize(&state, &wrong);
        assert_eq!(blind.title(), "", "non-member materializes no title");
        assert_eq!(blind.body(), "");
    }

    #[test]
    fn the_op_set_state_holds_no_plaintext() {
        // The daemon/relay stores exactly these bytes — they must not contain the
        // plaintext content (the confidentiality property, at the byte level).
        let key = ContentKey::generate();
        let secret = "Secret Title";
        let (state, _t, _b) = author_session(&key, 7);
        assert!(
            !state.windows(secret.len()).any(|w| w == secret.as_bytes()),
            "op-set ciphertext must not leak the plaintext title"
        );
        assert!(
            !state
                .windows("private body text".len())
                .any(|w| w == b"private body text"),
            "op-set ciphertext must not leak the plaintext body"
        );
    }

    #[test]
    fn op_ids_are_stable_and_distinct_per_ciphertext() {
        let key = ContentKey::generate();
        let mut node = KbNodeDoc::new_with_client_id("n1", "", "", &[], 1);
        let upd = node.set_title("X");
        let (id1, _) = seal_op(&[], &key, &upd, 1).unwrap();
        // op_id is stable for fixed bytes ...
        assert_eq!(op_id_of(b"abc"), op_id_of(b"abc"));
        // ... and sealing the SAME plaintext again yields a DISTINCT op (fresh nonce),
        // so it's a new op-set entry, never a silent overwrite.
        let (id2, _) = seal_op(&[], &key, &upd, 1).unwrap();
        assert_ne!(
            id1, id2,
            "fresh nonce ⇒ distinct op id for re-sealed plaintext"
        );
    }

    #[test]
    fn merge_is_set_union_and_idempotent() {
        let key = ContentKey::generate();
        // Shared base structure, then two members edit DIFFERENT fields concurrently.
        let base_node = KbNodeDoc::new_with_client_id("n1", "", "", &[], 1);
        let base_state = base_node.encode_state();
        let mut a = KbNodeDoc::from_bytes_with_client_id(&base_state, 1).unwrap();
        let mut b = KbNodeDoc::from_bytes_with_client_id(&base_state, 2).unwrap();

        let mut s = Vec::new();
        let (id0, o0) = seal_op(&s, &key, &base_state, 1).unwrap(); // op 0: structure
        s = merge(&s, &o0).unwrap();
        let (ida, oa) = seal_op(&s, &key, &a.set_title("from A"), 1).unwrap();
        let (idb, ob) = seal_op(&s, &key, &b.set_body("from B"), 2).unwrap();

        // Set-union: converges to the SAME 3-op set regardless of merge order +
        // idempotent re-apply.
        let m1 = merge(&merge(&s, &oa).unwrap(), &ob).unwrap();
        let m2 = merge(&merge(&s, &ob).unwrap(), &oa).unwrap();
        let m3 = merge(&m1, &oa).unwrap();
        for m in [&m1, &m2, &m3] {
            let ids = op_ids(m);
            assert!(ids.contains(&id0) && ids.contains(&ida) && ids.contains(&idb));
            assert_eq!(ids.len(), 3, "structure + the two distinct edits");
        }
        // The materialized text carries BOTH members' concurrent edits.
        let node = materialize(&m1, &key);
        assert_eq!(node.title(), "from A");
        assert_eq!(node.body(), "from B");
    }

    // ADR-040 #225: a re-seal-on-enable op 0 carries a LARGE pre-enable history (high SV
    // clock-total) under one client; a post-enable edit by a DIFFERENT client is a small
    // delta (low clock-total). `open_new_ops` sorts by ascending clock-total, which can place
    // that small edit BEFORE op 0 — so any reconstruction that treats `opened[0]` as the base
    // rebuilds from a mid-stream op. The materialized node must still round-trip through
    // `encode_state` → re-apply to a FRESH doc without a yrs gap — that is exactly what a
    // joiner's `reconcile_remote_node` does, and where the recovered-member join panicked.
    #[test]
    fn reseal_high_clock_op0_then_cross_client_edit_round_trips() {
        let key = ContentKey::generate();

        // A substantial pre-enable history under client 1 (high clock-total in op 0).
        let mut author = KbNodeDoc::new_with_client_id("n1", "", "", &[], 1);
        let _ = author.set_title("t1");
        let _ = author.set_body("body one");
        let _ = author.set_title("t2");
        let _ = author.set_body("body two — longer text to grow the clock and the text lineage");
        let pre_enable_state = author.encode_state();

        // op 0 = re-seal carrying the WHOLE pre-enable history.
        let mut state = Vec::new();
        let (_id0, o0) = seal_op(&state, &key, &pre_enable_state, 1).unwrap();
        state = merge(&state, &o0).unwrap();

        // A post-enable TEXT edit by a DIFFERENT client (2) — a small delta whose YText ops
        // depend on op 0's text positions. Low clock-total ⇒ sorts before op 0.
        let mut owner = KbNodeDoc::from_bytes_with_client_id(&pre_enable_state, 2).unwrap();
        let delta =
            owner.set_body("body two — longer text to grow the clock and the text lineage, POST");
        let (_id1, o1) = seal_op(&state, &key, &delta, 2).unwrap();
        state = merge(&state, &o1).unwrap();

        // Materialize (mirror of join-decrypt) → encode_state → re-apply to a fresh doc
        // (mirror of the joiner's reconcile_remote_node). This MUST NOT gap.
        let m = materialize(&state, &key);
        let encoded = m.encode_state();
        let reapplied = KbNodeDoc::from_bytes(&encoded).expect(
            "the reconstructed full state must re-apply to a fresh doc without a yrs gap (#225)",
        );
        assert_eq!(reapplied.title(), m.title(), "round-trip preserves title");
        assert_eq!(reapplied.body(), m.body(), "round-trip preserves body");
        assert_eq!(
            reapplied.body(),
            "body two — longer text to grow the clock and the text lineage, POST",
            "the post-enable edit by the second client materializes",
        );
    }
}
