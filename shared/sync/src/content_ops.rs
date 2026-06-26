//! Signed content operations (ADR-036) — peer-verifiable node-edit authorship.
//!
//! ADR-026 made **membership** peer-verifiable; this is its content-path sibling.
//! A content op is a yrs node-edit (`sync/update` for a `kb:{node}` doc) plus an
//! **authorship header** the editor signs with its Ed25519 identity key. On the
//! ADR-025 mesh a content op reaches a peer **relayed through** a daemon that peer
//! does not trust; without a signature a hostile relay could **mis-attribute** an
//! edit ("Bob wrote this") or inject content under a member's identity, and the
//! receiver — armed only with ADR-023's epoch fence, which proves *write-access* but
//! says nothing about *who* — could not reject it.
//!
//! So, mirroring [`crate::membership`]'s proven discipline: the author signs
//! [`ContentOp::canonical_bytes`] (version-tagged, NUL-separated header ‖ the raw yrs
//! payload), and every peer verifies on apply — `sig` valid, `author` bound to the
//! signing key, and (caller-supplied, ADR-026) the author was an authorized member
//! at the op's epoch. The daemon only **transports**: it neither authors nor needs
//! to be trusted for attribution (ADR-036 §D2 — honest-but-untrusted for content).
//!
//! Scope (ADR-036 §D4): this is the pragmatic, peer-verifiable slice — the existing
//! yrs SV (`base_sv`) + the ADR-023 epoch fence carry causal/replay safety, so there
//! is **no** second content-DAG here (that is the ADR-039 §B research endgame).
//! Confidentiality (a member/relay still *reads* plaintext) is ADR-037, built on this
//! same op substrate.

use crate::membership::fingerprint_of;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::kb::Role;
use crate::membership::ValidMember;
use std::collections::BTreeMap;

/// The signed authorship header of a content op (ADR-036 §D1). The yrs update
/// *payload* is carried alongside (in [`SignedContentOp`]) and bound into the
/// signature, not stored in the header — the header is what is small, structured,
/// and canonically encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentOp {
    /// The KB the edited node belongs to.
    pub kb_id: String,
    /// The node doc edited (e.g. `concept:buffer`), i.e. the `kb:{node_id}` doc.
    pub node_id: String,
    /// The author's yrs **state vector** at authoring time (ADR-022). Binds the op
    /// to the causal position it was produced from; a replay claiming a newer base
    /// fails the receiver's SV/epoch checks. Opaque bytes (hex-encoded in the
    /// canonical form so the NUL field separator stays unambiguous).
    pub base_sv: Vec<u8>,
    /// The author principal — `fingerprint_of(author_pubkey)`, `SHA256:…`
    /// (STANDARD_NO_PAD), byte-identical to the membership layer + `mae_mcp`.
    pub author: String,
    /// The author's ADR-023 authorization **epoch**, *signed* so every peer agrees
    /// (a per-peer token would break cross-peer `client_id` agreement — the ADR-026
    /// §2b-3 rule). The fence rejects an op authored under a stale (pre-grant /
    /// post-removal) epoch.
    pub epoch: u64,
    /// Issue time (unix seconds) — audit + tie-break, not a security boundary.
    pub issued_at: u64,
}

impl ContentOp {
    /// Deterministic canonical encoding — the exact bytes signed + verified:
    /// version-tagged, NUL-separated header fields, then the **raw yrs payload**
    /// appended last (so any payload mutation breaks the signature). Stable across
    /// platforms + serde versions; NUL never appears in a fingerprint, hex string,
    /// or decimal, so the header separation is unambiguous, and the payload — which
    /// *may* contain NUL — is unambiguous because it is the unframed remainder.
    pub fn canonical_bytes(&self, payload: &[u8]) -> Vec<u8> {
        fn field(b: &mut Vec<u8>, s: &str) {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        let mut b = Vec::new();
        field(&mut b, "maecontent/v1");
        field(&mut b, &self.kb_id);
        field(&mut b, &self.node_id);
        field(&mut b, &hex::encode(&self.base_sv));
        field(&mut b, &self.author);
        field(&mut b, &self.epoch.to_string());
        field(&mut b, &self.issued_at.to_string());
        b.extend_from_slice(payload);
        b
    }

    /// Sign `header ‖ payload` with the author's Ed25519 secret seed. Returns the
    /// 64-byte signature. Only editors hold an identity key (ADR-017/036 §D2).
    pub fn sign(&self, secret: &[u8; 32], payload: &[u8]) -> Vec<u8> {
        SigningKey::from_bytes(secret)
            .sign(&self.canonical_bytes(payload))
            .to_bytes()
            .to_vec()
    }

    /// Verify `sig` over `header ‖ payload` by the holder of `author_pubkey`. The
    /// caller must *separately* confirm `author_pubkey`'s fingerprint equals
    /// `self.author` — see [`fingerprint_matches`](Self::fingerprint_matches), or use
    /// [`SignedContentOp::verify_signed`] which does both.
    pub fn verify(&self, sig: &[u8], author_pubkey: &[u8; 32], payload: &[u8]) -> bool {
        let vk = match VerifyingKey::from_bytes(author_pubkey) {
            Ok(k) => k,
            Err(_) => return false,
        };
        let arr: [u8; 64] = match sig.try_into() {
            Ok(a) => a,
            Err(_) => return false,
        };
        vk.verify(&self.canonical_bytes(payload), &Signature::from_bytes(&arr))
            .is_ok()
    }

    /// True iff `pubkey`'s `SHA256:<base64>` fingerprint equals `self.author` —
    /// binds the verifying key to the claimed author principal (no impersonation).
    pub fn fingerprint_matches(&self, pubkey: &[u8; 32]) -> bool {
        fingerprint_of(pubkey) == self.author
    }
}

/// A [`ContentOp`] together with its yrs payload, signature, and the author's public
/// key — the self-attesting unit a peer receives + verifies. `author_pubkey` travels
/// with the op so any peer verifies locally with no external lookup; it is bound to
/// `op.author` by the fingerprint check in [`verify_signed`](Self::verify_signed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedContentOp {
    pub op: ContentOp,
    /// The yrs update bytes this op carries (the actual node edit).
    pub payload: Vec<u8>,
    pub sig: Vec<u8>,
    pub author_pubkey: [u8; 32],
}

/// Why a content op was refused admission — surfaced (ADR-024) rather than silently
/// dropped, so a forged/stale op is *visible*, not just absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    /// Signature invalid, or `author_pubkey` doesn't match the claimed `author`.
    BadSignature,
    /// `author` is not a current member of the KB (ADR-026 derived set).
    NotAMember,
    /// `author` is a member but lacks write capability (needs Editor+; a Viewer
    /// cannot author content).
    InsufficientRole,
    /// The op's `epoch` is stale vs the author's current authorization epoch —
    /// the ADR-023 fence: a pre-grant or post-removal/-rotation edit (#72 makes the
    /// epoch unpredictable, so this is not forgeable).
    StaleEpoch { op_epoch: u64, current_epoch: u64 },
}

impl SignedContentOp {
    /// Per-record cryptographic check (ADR-036 §D3.1): the signature verifies **and**
    /// the signing key belongs to the claimed author. Capability + epoch are layered
    /// on in [`admit`](Self::admit).
    pub fn verify_signed(&self) -> bool {
        self.op.fingerprint_matches(&self.author_pubkey)
            && self
                .op
                .verify(&self.sig, &self.author_pubkey, &self.payload)
    }

    /// Full peer-side admission decision (ADR-036 §D3), pure + trustless given the
    /// caller's ADR-026 derived membership (`members`, keyed by principal). Checks,
    /// in order:
    /// 1. crypto: [`verify_signed`](Self::verify_signed);
    /// 2. authority: `author` is a member with `Role::Editor`+ write capability;
    /// 3. fence (ADR-023): the op's `epoch` matches the author's **current** derived
    ///    authorization epoch — an op authored under a superseded grant is stale.
    ///
    /// `Ok(())` ⇒ admit + merge via the existing non-destructive `apply_update`
    /// (signing changes *admission*, never convergence). `Err` ⇒ reject + surface.
    /// The transport's own client_id-fence (`derive_kb_client_id`) remains a
    /// belt-and-suspenders check at the yrs layer; this is the authorship gate.
    pub fn admit(&self, members: &BTreeMap<String, ValidMember>) -> Result<(), AdmissionError> {
        if !self.verify_signed() {
            return Err(AdmissionError::BadSignature);
        }
        let member = members
            .get(&self.op.author)
            .ok_or(AdmissionError::NotAMember)?;
        if !member.role.includes(Role::Editor) {
            return Err(AdmissionError::InsufficientRole);
        }
        if member.epoch != self.op.epoch {
            return Err(AdmissionError::StaleEpoch {
                op_epoch: self.op.epoch,
                current_epoch: member.epoch,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (secret seed, pubkey bytes, fingerprint) for a deterministic test identity.
    fn ident(seed: u8) -> ([u8; 32], [u8; 32], String) {
        let secret = [seed; 32];
        let pubkey = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let fp = fingerprint_of(&pubkey);
        (secret, pubkey, fp)
    }

    fn sample(author: &str, epoch: u64) -> ContentOp {
        ContentOp {
            kb_id: "kb1".to_string(),
            node_id: "concept:buffer".to_string(),
            base_sv: vec![1, 0, 2, 0, 0], // arbitrary yrs SV bytes (incl. a NUL)
            author: author.to_string(),
            epoch,
            issued_at: 1_700_000_000,
        }
    }

    fn member(role: Role, epoch: u64) -> ValidMember {
        ValidMember {
            principal: "p".to_string(),
            role,
            can_invite: false,
            invited_by: "owner".to_string(),
            epoch,
        }
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        let (secret, pubkey, fp) = ident(1);
        let op = sample(&fp, 0);
        let payload = b"\x00yrs-update-with-nul";
        let sig = op.sign(&secret, payload);
        assert!(op.verify(&sig, &pubkey, payload));
        let signed = SignedContentOp {
            op,
            payload: payload.to_vec(),
            sig,
            author_pubkey: pubkey,
        };
        assert!(signed.verify_signed());
    }

    #[test]
    fn tampering_any_header_field_breaks_the_signature() {
        let (secret, pubkey, fp) = ident(2);
        let op = sample(&fp, 3);
        let payload = b"edit";
        let sig = op.sign(&secret, payload);

        // Each mutated field must fail verification (the signature covers them all).
        let mut t = op.clone();
        t.kb_id = "other".to_string();
        assert!(!t.verify(&sig, &pubkey, payload), "kb_id");
        let mut t = op.clone();
        t.node_id = "concept:other".to_string();
        assert!(!t.verify(&sig, &pubkey, payload), "node_id");
        let mut t = op.clone();
        t.base_sv = vec![9, 9];
        assert!(!t.verify(&sig, &pubkey, payload), "base_sv");
        let mut t = op.clone();
        t.epoch = 4;
        assert!(!t.verify(&sig, &pubkey, payload), "epoch");
        let mut t = op.clone();
        t.issued_at += 1;
        assert!(!t.verify(&sig, &pubkey, payload), "issued_at");
    }

    #[test]
    fn tampering_the_payload_breaks_the_signature() {
        let (secret, pubkey, fp) = ident(3);
        let op = sample(&fp, 0);
        let sig = op.sign(&secret, b"original");
        assert!(!op.verify(&sig, &pubkey, b"tampered"));
        assert!(op.verify(&sig, &pubkey, b"original"));
    }

    #[test]
    fn a_different_key_does_not_verify() {
        let (secret, _pub, fp) = ident(4);
        let op = sample(&fp, 0);
        let sig = op.sign(&secret, b"x");
        let other_pub = SigningKey::from_bytes(&[9u8; 32])
            .verifying_key()
            .to_bytes();
        assert!(!op.verify(&sig, &other_pub, b"x"));
    }

    #[test]
    fn forged_fingerprint_fails_verify_signed() {
        // Author claims a principal that is NOT the signing key's fingerprint: the
        // crypto sig is valid for the key, but the binding check rejects it (an
        // impersonation attempt — "this key speaks for someone else").
        let (secret, pubkey, _real_fp) = ident(5);
        let op = sample("SHA256:not-the-signing-keys-fingerprint", 0);
        let payload = b"x";
        let sig = op.sign(&secret, payload); // valid sig over the (forged-author) header
        assert!(
            op.verify(&sig, &pubkey, payload),
            "sig is valid for the key"
        );
        let signed = SignedContentOp {
            op,
            payload: payload.to_vec(),
            sig,
            author_pubkey: pubkey,
        };
        assert!(
            !signed.verify_signed(),
            "fingerprint binding rejects the impersonation"
        );
    }

    #[test]
    fn admit_accepts_a_valid_editor_at_current_epoch() {
        let (secret, pubkey, fp) = ident(6);
        let op = sample(&fp, 7);
        let payload = b"edit";
        let sig = op.sign(&secret, payload);
        let signed = SignedContentOp {
            op,
            payload: payload.to_vec(),
            sig,
            author_pubkey: pubkey,
        };
        let mut members = BTreeMap::new();
        members.insert(fp.clone(), member(Role::Editor, 7));
        assert_eq!(signed.admit(&members), Ok(()));
        // An owner (⊇ editor) is likewise admitted.
        members.insert(fp.clone(), member(Role::Owner, 7));
        assert_eq!(signed.admit(&members), Ok(()));
    }

    #[test]
    fn admit_rejects_non_member_viewer_and_stale_epoch() {
        let (secret, pubkey, fp) = ident(7);
        let op = sample(&fp, 5);
        let payload = b"edit";
        let sig = op.sign(&secret, payload);
        let signed = SignedContentOp {
            op,
            payload: payload.to_vec(),
            sig,
            author_pubkey: pubkey,
        };

        // Not in the derived set ⇒ NotAMember.
        let empty = BTreeMap::new();
        assert_eq!(signed.admit(&empty), Err(AdmissionError::NotAMember));

        // A Viewer cannot author content ⇒ InsufficientRole.
        let mut viewer = BTreeMap::new();
        viewer.insert(fp.clone(), member(Role::Viewer, 5));
        assert_eq!(signed.admit(&viewer), Err(AdmissionError::InsufficientRole));

        // Editor but the op's epoch is behind the member's current grant ⇒ stale.
        let mut bumped = BTreeMap::new();
        bumped.insert(fp.clone(), member(Role::Editor, 9));
        assert_eq!(
            signed.admit(&bumped),
            Err(AdmissionError::StaleEpoch {
                op_epoch: 5,
                current_epoch: 9
            })
        );
    }

    #[test]
    fn admit_rejects_a_tampered_payload_as_bad_signature() {
        // A relay mutates the payload after the author signed: verify_signed fails,
        // so admission is BadSignature even though `author` IS a valid editor.
        let (secret, pubkey, fp) = ident(8);
        let op = sample(&fp, 0);
        let sig = op.sign(&secret, b"original");
        let signed = SignedContentOp {
            op,
            payload: b"tampered-by-relay".to_vec(),
            sig,
            author_pubkey: pubkey,
        };
        let mut members = BTreeMap::new();
        members.insert(fp.clone(), member(Role::Editor, 0));
        assert_eq!(signed.admit(&members), Err(AdmissionError::BadSignature));
    }

    #[test]
    fn fingerprint_binding_matches_membership_format() {
        // Same fingerprint discipline as the membership layer (cross-crate stable).
        let (_s, pubkey, fp) = ident(10);
        assert!(fp.starts_with("SHA256:"));
        assert_eq!(fingerprint_of(&pubkey), fp);
    }
}
