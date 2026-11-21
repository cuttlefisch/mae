//! The signed authorship header of a KB content op (ADR-036 D1), and the one
//! canonical encoding every signer and verifier uses.
//!
//! A content op is a yrs node edit (`sync/update` for a `kbn:{kb}:{node}`
//! document, or `kb/node_update`) plus this header, signed by the author's
//! member key. The daemon verifies it (`mae_sync::content_ops::SignedContentOp`,
//! which re-exports [`ContentOp`] from here); the native editor signs with its
//! identity key; a browser signs through mae-crypto-wasm's `sign_op`. All three
//! call [`ContentOp::canonical_bytes`] in this file, so they cannot disagree
//! about what was signed (ADR-036 F5).
//!
//! @ai-caution: [security] `canonical_bytes` is a wire format: every signed op
//! ever written verifies against it. Changing a field, its order, its encoding
//! or the `maecontent/v1` tag invalidates them all. A new format is a new tag
//! (`maecontent/v2`) verified beside the old one, never an edit of this one.
//! `tests::the_canonical_bytes_and_signature_are_pinned` holds the vector.

use crate::member::{fingerprint, MemberKey};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// The domain tag that opens [`ContentOp::canonical_bytes`]. Distinct from the
/// identity-binding challenge's `mae-identity-binding-v1`, so a member key's
/// signature over one can never be presented as the other.
pub const CONTENT_OP_DOMAIN: &str = "maecontent/v1";

/// The signed authorship header of a content op (ADR-036 §D1). The yrs update
/// *payload* is carried alongside it and bound into the signature, not stored
/// in the header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentOp {
    /// The KB the edited node belongs to.
    pub kb_id: String,
    /// The node edited, i.e. the node of the `kbn:{kb_id}:{node_id}` document.
    pub node_id: String,
    /// The author's yrs **state vector** at authoring time (ADR-022). Binds the op
    /// to the causal position it was produced from. Opaque bytes (hex-encoded in
    /// the canonical form so the NUL field separator stays unambiguous).
    pub base_sv: Vec<u8>,
    /// The author principal: the fingerprint of the signing key, `SHA256:…`
    /// (STANDARD_NO_PAD), byte-identical to the membership layer and `mae_mcp`.
    pub author: String,
    /// The author's ADR-023 authorization **epoch**, signed so every peer agrees.
    /// The fence rejects an op authored under a stale epoch.
    pub epoch: u64,
    /// Issue time (unix seconds): audit and tie-break, not a security boundary.
    pub issued_at: u64,
}

/// Why a header cannot be signed: a field the canonical encoding cannot carry
/// unambiguously.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderError {
    /// `kb_id` or `node_id` is empty.
    EmptyField(&'static str),
    /// A NUL byte in `kb_id` or `node_id`: NUL separates the encoding's fields.
    NulInField(&'static str),
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderError::EmptyField(k) => write!(f, "{k} is empty"),
            HeaderError::NulInField(k) => write!(f, "{k} contains a NUL byte"),
        }
    }
}

impl std::error::Error for HeaderError {}

fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

impl ContentOp {
    /// Deterministic canonical encoding, the exact bytes signed and verified:
    /// the domain tag, then the NUL-separated header fields, then the **raw yrs
    /// payload** appended last (so any payload change breaks the signature).
    /// NUL never appears in a fingerprint, a hex string or a decimal, and
    /// [`check`](Self::check) refuses it in the ids, so the header separation is
    /// unambiguous; the payload, which may contain NUL, is the unframed remainder.
    pub fn canonical_bytes(&self, payload: &[u8]) -> Vec<u8> {
        fn field(b: &mut Vec<u8>, s: &str) {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        let mut b = Vec::new();
        field(&mut b, CONTENT_OP_DOMAIN);
        field(&mut b, &self.kb_id);
        field(&mut b, &self.node_id);
        field(&mut b, &hex_lower(&self.base_sv));
        field(&mut b, &self.author);
        field(&mut b, &self.epoch.to_string());
        field(&mut b, &self.issued_at.to_string());
        b.extend_from_slice(payload);
        b
    }

    /// Whether every id field can be encoded unambiguously. A signer checks this
    /// before signing, and [`verify`](Self::verify) refuses a header that fails
    /// it, whatever its signature.
    pub fn check(&self) -> Result<(), HeaderError> {
        for (name, v) in [("kb_id", &self.kb_id), ("node_id", &self.node_id)] {
            if v.is_empty() {
                return Err(HeaderError::EmptyField(name));
            }
            if v.as_bytes().contains(&0) {
                return Err(HeaderError::NulInField(name));
            }
        }
        Ok(())
    }

    /// A header authored by `key`: `author` is the key's own fingerprint, never
    /// a caller's claim.
    pub fn for_key(
        key: &MemberKey,
        kb_id: &str,
        node_id: &str,
        base_sv: &[u8],
        epoch: u64,
        issued_at: u64,
    ) -> Self {
        ContentOp {
            kb_id: kb_id.to_string(),
            node_id: node_id.to_string(),
            base_sv: base_sv.to_vec(),
            author: key.fingerprint(),
            epoch,
            issued_at,
        }
    }

    /// Sign `header ‖ payload` with a held member key (the browser path).
    pub fn sign_with(&self, key: &MemberKey, payload: &[u8]) -> [u8; 64] {
        key.sign(&self.canonical_bytes(payload))
    }

    /// Sign `header ‖ payload` with the author's Ed25519 secret seed (the editor
    /// path). Returns the 64-byte signature.
    pub fn sign(&self, secret: &[u8; 32], payload: &[u8]) -> Vec<u8> {
        SigningKey::from_bytes(secret)
            .sign(&self.canonical_bytes(payload))
            .to_bytes()
            .to_vec()
    }

    /// Verify `sig` over `header ‖ payload` by the holder of `author_pubkey`. The
    /// caller must *separately* confirm `author_pubkey`'s fingerprint equals
    /// `self.author`: see [`fingerprint_matches`](Self::fingerprint_matches).
    pub fn verify(&self, sig: &[u8], author_pubkey: &[u8; 32], payload: &[u8]) -> bool {
        // A header the encoding cannot carry unambiguously never verifies: two
        // NUL-split (kb_id, node_id) pairs encode to the same bytes, so one
        // signature would cover both.
        if self.check().is_err() {
            return false;
        }
        let Ok(vk) = VerifyingKey::from_bytes(author_pubkey) else {
            return false;
        };
        let Ok(arr) = <[u8; 64]>::try_from(sig) else {
            return false;
        };
        vk.verify(&self.canonical_bytes(payload), &Signature::from_bytes(&arr))
            .is_ok()
    }

    /// True iff `pubkey`'s fingerprint equals `self.author`: binds the verifying
    /// key to the claimed author principal (no impersonation).
    pub fn fingerprint_matches(&self, pubkey: &[u8; 32]) -> bool {
        fingerprint(pubkey) == self.author
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> MemberKey {
        MemberKey::from_seed(&[0x5a; 32])
    }

    fn header(k: &MemberKey) -> ContentOp {
        ContentOp::for_key(
            k,
            "kb-alpha",
            "concept:buffer",
            &[1, 2, 0, 255],
            7,
            1_790_000_000,
        )
    }

    /// The test vector browser and native signers pin. If this changes, every
    /// signed op ever written stops verifying: see the module's @ai-caution.
    #[test]
    fn the_canonical_bytes_and_signature_are_pinned() {
        let k = key();
        let h = header(&k);
        let payload = [0u8, 1, 2, 3, 0, 9];
        let bytes = h.canonical_bytes(&payload);
        let mut want = Vec::new();
        for f in [
            "maecontent/v1",
            "kb-alpha",
            "concept:buffer",
            "010200ff",
            &k.fingerprint(),
            "7",
            "1790000000",
        ] {
            want.extend_from_slice(f.as_bytes());
            want.push(0);
        }
        want.extend_from_slice(&payload);
        assert_eq!(bytes, want);
        assert_eq!(
            hex_lower(&h.sign_with(&k, &payload)),
            PINNED_SIGNATURE,
            "the pinned signature over the vector moved"
        );
    }

    /// Ed25519 (RFC 8032) is deterministic: seed 0x5a x 32 over the vector above.
    const PINNED_SIGNATURE: &str = "f98071fb0ab87e6107a5e0835b3beadc3aa532bee4de847b6dba6a0b1ba67a6fc8ee7c45935c5d3cb25143207acb1c39ab20f620bf16d135782318d049a87702";

    #[test]
    fn a_signature_verifies_only_for_its_exact_header_payload_and_key() {
        let k = key();
        let h = header(&k);
        let payload = b"yrs update bytes".to_vec();
        let sig = h.sign_with(&k, &payload);
        let pk = k.public_key();
        assert!(h.verify(&sig, &pk, &payload));
        assert!(h.fingerprint_matches(&pk));
        // Any changed field, payload byte or key fails.
        let mut other = h.clone();
        other.node_id = "concept:other".into();
        assert!(!other.verify(&sig, &pk, &payload));
        let mut other = h.clone();
        other.kb_id = "kb-beta".into();
        assert!(!other.verify(&sig, &pk, &payload));
        let mut other = h.clone();
        other.epoch += 1;
        assert!(!other.verify(&sig, &pk, &payload));
        let mut other = h.clone();
        other.base_sv.push(0);
        assert!(!other.verify(&sig, &pk, &payload));
        let mut tampered = payload.clone();
        tampered[0] ^= 1;
        assert!(!h.verify(&sig, &pk, &tampered));
        let stranger = MemberKey::from_seed(&[0x11; 32]);
        assert!(!h.verify(&sig, &stranger.public_key(), &payload));
        assert!(!h.fingerprint_matches(&stranger.public_key()));
    }

    #[test]
    fn the_seed_and_member_key_paths_sign_identically() {
        let k = key();
        let h = header(&k);
        let payload = b"p";
        assert_eq!(
            h.sign(&[0x5a; 32], payload),
            h.sign_with(&k, payload).to_vec()
        );
    }

    #[test]
    fn ids_that_cannot_be_encoded_unambiguously_are_refused() {
        let k = key();
        let ok = header(&k);
        assert_eq!(ok.check(), Ok(()));
        let mut h = ok.clone();
        h.kb_id.clear();
        assert_eq!(h.check(), Err(HeaderError::EmptyField("kb_id")));
        let mut h = ok.clone();
        h.node_id = "a\0b".into();
        assert_eq!(h.check(), Err(HeaderError::NulInField("node_id")));
        // Why NUL matters: without the check, two different (kb, node) splits
        // would encode to the same bytes, so one signature would cover both.
        let a = ContentOp {
            kb_id: "x\0y".into(),
            node_id: "z".into(),
            ..ok.clone()
        };
        let b = ContentOp {
            kb_id: "x".into(),
            node_id: "y\0z".into(),
            ..ok
        };
        assert_eq!(a.canonical_bytes(b""), b.canonical_bytes(b""));
    }

    #[test]
    fn a_nul_split_header_never_verifies_even_with_a_valid_signature() {
        // The editor's seed path signs without `check`: sign a NUL-bearing node
        // id, then present the signature under the other split of the same bytes.
        let seed = [0x5a; 32];
        let k = MemberKey::from_seed(&seed);
        let signed = ContentOp {
            kb_id: "x".into(),
            node_id: "y\0z".into(),
            ..header(&k)
        };
        let other = ContentOp {
            kb_id: "x\0y".into(),
            node_id: "z".into(),
            ..header(&k)
        };
        let sig = signed.sign(&seed, b"p");
        assert_eq!(signed.canonical_bytes(b"p"), other.canonical_bytes(b"p"));
        assert!(
            !other.verify(&sig, &k.public_key(), b"p"),
            "the other split"
        );
        assert!(
            !signed.verify(&sig, &k.public_key(), b"p"),
            "nor the one signed"
        );
    }

    #[test]
    fn hex_is_lower_case_two_digits_per_byte() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(hex_lower(&[]), "");
    }
}
