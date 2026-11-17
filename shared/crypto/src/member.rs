//! A MAE member key: an Ed25519 key held as its 32-byte seed, with the
//! public forms MAE uses for it.
//!
//! The public key travels as standard base64 with padding (RFC 4648 §4) of
//! its 32 bytes, and the fingerprint is `SHA256:` + base64 without padding
//! of SHA-256 over those 32 bytes, both exactly as `mae_mcp::identity`
//! writes them (`PublicKey::encoded`, `PublicKey::fingerprint`; the parity
//! test is `shared/mcp/tests/member_key_parity.rs`).
//!
//! @ai-caution: [security] The seed is the member's whole access: it signs
//! membership and content ops, and the content-key wrap key is derived from
//! it. [`MemberKey::seed`] is the only way out, for a caller that asks to
//! export it or to seal it in a backup; nothing here logs, formats or
//! serialises it ([`MemberKey`]'s `Debug` prints the fingerprint only).

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::content_crypto::{self, ContentKey, CryptoError};
use crate::rng::{self, RandomUnavailable};

/// Ed25519 member key. The signing key zeroizes itself on drop.
pub struct MemberKey {
    signing: SigningKey,
}

impl MemberKey {
    /// A fresh key from 32 random bytes ([`crate::rng`]: the host's CSPRNG in
    /// the WASM module). No source, no key.
    pub fn generate() -> Result<Self, RandomUnavailable> {
        let mut seed = Zeroizing::new([0u8; 32]);
        rng::fill(&mut seed[..])?;
        Ok(Self::from_seed(&seed))
    }

    /// The key whose seed is `seed` (as `Identity::from_seed` builds it).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// The secret seed, for an explicit export or a backup. Wiped on drop.
    pub fn seed(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.signing.to_bytes())
    }

    /// The 32 public-key bytes.
    pub fn public_key(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    /// The public key as MAE writes it on the wire: standard base64, padded.
    pub fn public_key_b64(&self) -> String {
        STANDARD.encode(self.public_key())
    }

    /// `SHA256:<base64, no padding>` of the public key.
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.public_key())
    }

    /// The 64-byte Ed25519 signature over `msg`.
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.signing.sign(msg).to_bytes()
    }

    /// The X25519 wrap public key this member publishes for content-key
    /// wraps (ADR-041; `content_crypto::wrap_public_for`).
    pub fn wrap_public(&self) -> [u8; 32] {
        content_crypto::wrap_public_for(&self.seed())
    }

    /// Opens a content key wrapped to [`Self::wrap_public`].
    pub fn unwrap_content_key(&self, wrapped: &[u8]) -> Result<ContentKey, CryptoError> {
        content_crypto::unwrap_as_member(wrapped, &self.seed())
    }
}

impl std::fmt::Debug for MemberKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MemberKey({})", self.fingerprint())
    }
}

/// The fingerprint of a 32-byte Ed25519 public key: `SHA256:` + base64
/// (standard alphabet, no padding) of its SHA-256.
pub fn fingerprint(public_key: &[u8; 32]) -> String {
    format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(public_key))
    )
}

/// A public key in MAE's wire form, decoded: exactly 32 bytes of standard,
/// padded base64 that are a valid Ed25519 point. `None` otherwise.
pub fn decode_public_key(b64: &str) -> Option<[u8; 32]> {
    let bytes: [u8; 32] = STANDARD.decode(b64).ok()?.try_into().ok()?;
    ed25519_dalek::VerifyingKey::from_bytes(&bytes).ok()?;
    Some(bytes)
}

/// Whether `signature` is a valid Ed25519 signature by `public_key` over
/// `msg` (strict verification: no malleable or small-order forms).
pub fn verify(public_key: &[u8; 32], msg: &[u8], signature: &[u8]) -> bool {
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let Ok(sig) = <[u8; 64]>::try_from(signature) else {
        return false;
    };
    key.verify_strict(msg, &ed25519_dalek::Signature::from_bytes(&sig))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_are_distinct_and_sign_verifiably() {
        let keys: Vec<MemberKey> = (0..8).map(|_| MemberKey::generate().unwrap()).collect();
        for (i, k) in keys.iter().enumerate() {
            let msg = format!("message {i}");
            let sig = k.sign(msg.as_bytes());
            assert!(verify(&k.public_key(), msg.as_bytes(), &sig));
            for (j, other) in keys.iter().enumerate() {
                if i != j {
                    assert_ne!(k.public_key(), other.public_key());
                    assert!(!verify(&other.public_key(), msg.as_bytes(), &sig));
                }
            }
            let mut tampered = sig;
            tampered[i % 64] ^= 1;
            assert!(!verify(&k.public_key(), msg.as_bytes(), &tampered));
            assert!(!verify(&k.public_key(), b"another message", &sig));
        }
    }

    #[test]
    fn a_seed_round_trips_to_the_same_key() {
        let k = MemberKey::generate().unwrap();
        let again = MemberKey::from_seed(&k.seed());
        assert_eq!(again.public_key(), k.public_key());
        assert_eq!(again.fingerprint(), k.fingerprint());
        assert_eq!(again.sign(b"m"), k.sign(b"m"), "Ed25519 is deterministic");
    }

    /// The public forms: padded base64 of 32 bytes (44 chars), and a
    /// fingerprint of 43 unpadded base64 chars that decodes to the SHA-256.
    #[test]
    fn public_forms_are_the_wire_forms() {
        let k = MemberKey::generate().unwrap();
        let b64 = k.public_key_b64();
        assert_eq!(b64.len(), 44);
        assert!(b64.ends_with('='));
        assert_eq!(decode_public_key(&b64), Some(k.public_key()));
        let fp = k.fingerprint();
        let body = fp.strip_prefix("SHA256:").unwrap();
        assert_eq!(body.len(), 43);
        assert!(!body.contains('='));
        let digest = STANDARD_NO_PAD.decode(body).unwrap();
        assert_eq!(digest, Sha256::digest(k.public_key()).to_vec());
    }

    #[test]
    fn malformed_public_keys_are_refused() {
        let k = MemberKey::generate().unwrap();
        let b64 = k.public_key_b64();
        assert_eq!(decode_public_key(&b64[..43]), None, "unpadded");
        assert_eq!(decode_public_key(&STANDARD.encode([1u8; 31])), None);
        assert_eq!(decode_public_key(&STANDARD.encode([1u8; 33])), None);
        assert_eq!(decode_public_key("not base64!"), None);
        // 32 bytes that are not an Ed25519 point (about half of all 32-byte
        // strings): refused, while valid ones decode.
        let (mut bad, mut good) = (0, 0);
        for i in 0..200u8 {
            let mut b = [0u8; 32];
            rng::fill(&mut b).unwrap();
            b[0] = i;
            let valid = ed25519_dalek::VerifyingKey::from_bytes(&b).is_ok();
            let decoded = decode_public_key(&STANDARD.encode(b));
            assert_eq!(decoded.is_some(), valid, "{b:?}");
            if valid {
                good += 1
            } else {
                bad += 1
            }
        }
        assert!(bad > 0 && good > 0, "both kinds were tried");
    }

    #[test]
    fn debug_shows_no_secret() {
        let k = MemberKey::generate().unwrap();
        let shown = format!("{k:?}");
        let seed = k.seed();
        for form in [
            STANDARD.encode(*seed),
            STANDARD_NO_PAD.encode(*seed),
            seed.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        ] {
            assert!(!shown.contains(&form));
        }
        assert!(shown.contains(&k.fingerprint()));
    }

    #[test]
    fn the_wrap_key_is_the_one_content_crypto_derives_and_unwraps() {
        let k = MemberKey::generate().unwrap();
        assert_eq!(k.wrap_public(), content_crypto::wrap_public_for(&k.seed()));
        let ck = ContentKey::generate();
        let wrapped = content_crypto::wrap_to_member(&ck, &k.wrap_public()).unwrap();
        assert_eq!(k.unwrap_content_key(&wrapped).unwrap(), ck);
        let other = MemberKey::generate().unwrap();
        assert_eq!(
            other.unwrap_content_key(&wrapped),
            Err(CryptoError::Decrypt)
        );
    }
}
