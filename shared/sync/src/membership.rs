//! Signed, hash-chained membership operations (ADR-026) — the capability-based
//! membership protocol for the P2P mesh.
//!
//! Each membership mutation (admit / remove / role-change / revoke) is an
//! **Ed25519-signed op** whose validity *any* peer derives locally, without
//! trusting a relaying daemon. The design composes prior art:
//! - **UCAN** — a grant names its issuer (`author`) + subject and carries a
//!   timebox (`expires_at`); the `can_invite` capability is a delegation.
//! - **Keybase sigchains** — ops are **hash-chained** (`prev_hash`), so any
//!   reorder/omission/forgery breaks the chain.
//! - **p2panda-auth** — an op is valid only if the author held the capability at
//!   its causal position; concurrent conflicts resolve deterministically.
//!
//! This module is the cryptographic + canonical-encoding foundation: the
//! [`MembershipOp`] struct, its deterministic [`MembershipOp::canonical_bytes`]
//! (what is signed + hashed), and sign/verify/chain. Validity *derivation*
//! (timebox, revocation, capability, the resolver) and the `KbCollectionDoc`
//! wiring build on top of this in later slices.

use crate::kb::Role;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

/// The membership change an op performs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipAction {
    /// Admit `subject` at `role` (with optional `can_invite`), by `author`.
    Admit,
    /// Remove `subject` from the KB.
    Remove,
    /// Change `subject`'s role.
    SetRole,
    /// Revoke an outstanding invite / admission for `subject`.
    Revoke,
}

impl MembershipAction {
    pub fn as_str(self) -> &'static str {
        match self {
            MembershipAction::Admit => "admit",
            MembershipAction::Remove => "remove",
            MembershipAction::SetRole => "set_role",
            MembershipAction::Revoke => "revoke",
        }
    }
    pub fn parse(s: &str) -> Option<MembershipAction> {
        match s {
            "admit" => Some(MembershipAction::Admit),
            "remove" => Some(MembershipAction::Remove),
            "set_role" => Some(MembershipAction::SetRole),
            "revoke" => Some(MembershipAction::Revoke),
            _ => None,
        }
    }
}

/// A signed membership operation. The [`canonical_bytes`](Self::canonical_bytes)
/// are what get signed + hash-chained; the signature proves `author` authored it.
/// Validity (capability-at-epoch, timebox, revocation, cascade) is derived
/// per-peer (ADR-026), not stored as a verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipOp {
    /// The KB this op mutates.
    pub kb_id: String,
    /// What the op does.
    pub action: MembershipAction,
    /// The principal acted on (Ed25519 key fingerprint, `SHA256:…`).
    pub subject: String,
    /// The granted role (Admit / SetRole).
    pub role: Option<Role>,
    /// Whether the op grants `subject` the delegable invite capability (Admit).
    pub can_invite: bool,
    /// The issuer principal (= the `invited_by` audit field) — the fingerprint of
    /// the key that signs this op.
    pub author: String,
    /// Issue time (unix seconds) — anchors causal/timebox checks.
    pub issued_at: u64,
    /// Expiry (unix seconds); `None` = no timebox.
    pub expires_at: Option<u64>,
    /// Hex of the previous op's [`chain_hash`](Self::chain_hash); `""` = genesis.
    pub prev_hash: String,
}

impl MembershipOp {
    /// Deterministic canonical encoding — the exact bytes that are signed +
    /// hashed. Version-tagged + NUL-separated so it is stable across platforms
    /// and serde versions (no field-ordering ambiguity). NUL never appears in a
    /// fingerprint, role, or decimal, so the separation is unambiguous.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn field(b: &mut Vec<u8>, s: &str) {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        let mut b = Vec::new();
        field(&mut b, "maememb/v1");
        field(&mut b, &self.kb_id);
        field(&mut b, self.action.as_str());
        field(&mut b, &self.subject);
        field(&mut b, self.role.map(|r| r.as_str()).unwrap_or(""));
        field(&mut b, if self.can_invite { "1" } else { "0" });
        field(&mut b, &self.author);
        field(&mut b, &self.issued_at.to_string());
        field(
            &mut b,
            &self.expires_at.map(|e| e.to_string()).unwrap_or_default(),
        );
        field(&mut b, &self.prev_hash);
        b
    }

    /// Sign with the author's Ed25519 secret seed (the daemon's own identity, for
    /// a KB it owns/manages). Returns the 64-byte signature.
    pub fn sign(&self, secret: &[u8; 32]) -> Vec<u8> {
        SigningKey::from_bytes(secret)
            .sign(&self.canonical_bytes())
            .to_bytes()
            .to_vec()
    }

    /// Verify `sig` was produced over this op by the holder of `author_pubkey`.
    /// (The caller must separately confirm `author_pubkey`'s fingerprint equals
    /// `self.author` — see [`fingerprint_matches`](Self::fingerprint_matches).)
    pub fn verify(&self, sig: &[u8], author_pubkey: &[u8; 32]) -> bool {
        let vk = match VerifyingKey::from_bytes(author_pubkey) {
            Ok(k) => k,
            Err(_) => return false,
        };
        let arr: [u8; 64] = match sig.try_into() {
            Ok(a) => a,
            Err(_) => return false,
        };
        vk.verify(&self.canonical_bytes(), &Signature::from_bytes(&arr))
            .is_ok()
    }

    /// True iff `pubkey`'s `SHA256:<base64>` fingerprint equals `self.author` —
    /// binds the verifying key to the claimed author principal.
    pub fn fingerprint_matches(&self, pubkey: &[u8; 32]) -> bool {
        fingerprint_of(pubkey) == self.author
    }

    /// The hash this op contributes as the *next* op's `prev_hash`:
    /// `hex(sha256(canonical_bytes ‖ sig))` — Keybase-style tamper-evident
    /// chaining (binds the signature, not just the payload).
    pub fn chain_hash(&self, sig: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(self.canonical_bytes());
        h.update(sig);
        hex::encode(h.finalize())
    }
}

/// The `SHA256:<base64>` fingerprint of an Ed25519 public key — the membership
/// **principal**. Matches `mae_mcp::identity::PublicKey::fingerprint()` so a
/// member's principal is identical whether derived here or there.
pub fn fingerprint_of(pubkey: &[u8; 32]) -> String {
    use base64::Engine;
    // MUST match `mae_mcp::identity::PublicKey::fingerprint()` exactly
    // (STANDARD_NO_PAD) so a principal is byte-identical across crates.
    let digest = Sha256::digest(pubkey);
    format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_op(prev_hash: &str) -> MembershipOp {
        MembershipOp {
            kb_id: "concept:x".into(),
            action: MembershipAction::Admit,
            subject: "SHA256:bob".into(),
            role: Some(Role::Editor),
            can_invite: false,
            author: "SHA256:owner".into(),
            issued_at: 1_700_000_000,
            expires_at: Some(1_700_086_400),
            prev_hash: prev_hash.into(),
        }
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        let secret = [7u8; 32];
        let pubkey = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let op = sample_op("");

        let sig = op.sign(&secret);
        assert_eq!(sig.len(), 64);
        assert!(op.verify(&sig, &pubkey), "a fresh signature must verify");
    }

    #[test]
    fn tampering_any_field_breaks_the_signature() {
        let secret = [7u8; 32];
        let pubkey = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let op = sample_op("");
        let sig = op.sign(&secret);

        // Each mutation must invalidate the signature (canonical bytes change).
        let mut t = op.clone();
        t.role = Some(Role::Owner); // privilege escalation attempt
        assert!(!t.verify(&sig, &pubkey));

        let mut t = op.clone();
        t.subject = "SHA256:mallory".into();
        assert!(!t.verify(&sig, &pubkey));

        let mut t = op.clone();
        t.expires_at = None; // strip the timebox
        assert!(!t.verify(&sig, &pubkey));

        let mut t = op.clone();
        t.can_invite = true; // self-grant the invite capability
        assert!(!t.verify(&sig, &pubkey));
    }

    #[test]
    fn a_different_key_does_not_verify() {
        let op = sample_op("");
        let sig = op.sign(&[7u8; 32]);
        let other_pub = SigningKey::from_bytes(&[9u8; 32])
            .verifying_key()
            .to_bytes();
        assert!(!op.verify(&sig, &other_pub), "wrong author key must fail");
        // A malformed signature also fails (not panics).
        assert!(!op.verify(b"too short", &other_pub));
    }

    #[test]
    fn fingerprint_binding_matches_mcp_format() {
        let pubkey = SigningKey::from_bytes(&[3u8; 32])
            .verifying_key()
            .to_bytes();
        let fp = fingerprint_of(&pubkey);
        assert!(fp.starts_with("SHA256:"));
        let op = MembershipOp {
            author: fp.clone(),
            ..sample_op("")
        };
        assert!(op.fingerprint_matches(&pubkey));
        // A different key's fingerprint does not match the claimed author.
        let other = SigningKey::from_bytes(&[4u8; 32])
            .verifying_key()
            .to_bytes();
        assert!(!op.fingerprint_matches(&other));
    }

    #[test]
    fn chain_hash_is_deterministic_and_binds_signature() {
        let op = sample_op("");
        let sig = op.sign(&[7u8; 32]);
        let h1 = op.chain_hash(&sig);
        let h2 = op.chain_hash(&sig);
        assert_eq!(h1, h2, "chain hash is deterministic");
        assert_eq!(h1.len(), 64, "sha256 hex");
        // A different signature ⇒ a different chain hash (binds the sig).
        let sig2 = op.sign(&[8u8; 32]);
        assert_ne!(op.chain_hash(&sig2), h1);
        // The next op linking to this one carries h1 as prev_hash.
        let next = sample_op(&h1);
        assert_eq!(next.prev_hash, h1);
    }
}
