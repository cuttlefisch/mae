//! ADR-037 content-encryption primitives — the pure crypto FOUNDATION (#131).
//!
//! This is the confidentiality counterpart to [`crate::content_ops`] (integrity).
//! ADR-037 encrypts content-op payloads with a **per-KB symmetric content key**,
//! distributed to members through the ADR-026 membership op-log, so a relaying /
//! hosting daemon — or the hub server — can carry a KB it **cannot read**
//! (ciphertext only). The daemon stays **key-blind**: it verifies the ADR-036
//! signature over the *ciphertext* and relays it, never holding the content key.
//!
//! This module is JUST the primitives — pure, transport-agnostic, no daemon/editor
//! wiring (see the follow-up op-set design for that):
//! - **AEAD** ([`encrypt`]/[`decrypt`], XChaCha20-Poly1305) for the content key.
//! - **Key wrap** ([`wrap_to_member`]/[`unwrap_as_member`]) — a sealed box (ephemeral
//!   X25519 ECDH → SHA-256 KDF → AEAD) that wraps the content key to a member's
//!   **published X25519 wrap key** ([`wrap_public_for`], ADR-041 / #158 I1 — a dedicated,
//!   domain-separated subkey, NOT the Ed25519 identity), so a member's `Admit` op in the
//!   signed log delivers the key with no key server (§D2). Signing and key-exchange use
//!   separate keys; the recipient publishes its wrap key (a sender can't derive it from
//!   the Ed25519 public key alone).
//!
//! Encrypt-then-sign (§D1): the caller encrypts the payload *before* signing it with
//! [`crate::content_ops`], so a peer verifies authorship + authorization *before*
//! decrypting, and a relay verifies integrity without the key. `SignedContentOp`'s
//! `payload` is opaque bytes — it transparently holds ciphertext, no struct change.

use chacha20poly1305::aead::Aead;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256, Sha512};
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// XChaCha20-Poly1305 nonce length (24 bytes — large enough for random nonces to be
/// collision-safe without a counter, the reason for the *X*-variant).
const NONCE_LEN: usize = 24;
/// X25519 public-key / shared-secret length.
const X25519_LEN: usize = 32;

/// A per-KB symmetric content key (ADR-037 §D1). 32 bytes for XChaCha20-Poly1305.
/// `ZeroizeOnDrop` (#156 F9) wipes the bytes on drop via a non-elidable volatile write
/// and a compiler fence — replacing the old best-effort manual `Drop`, which the
/// optimizer could elide. `Clone` is retained (callers cache/wrap the key); each clone
/// wipes on its own drop.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct ContentKey([u8; 32]);

impl ContentKey {
    /// A fresh random content key (`rand::random` — the version-stable top-level API,
    /// matching the epoch-token RNG in `kb.rs`).
    pub fn generate() -> Self {
        ContentKey(rand::random::<[u8; 32]>())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        ContentKey(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

// Deliberately no Debug/Display: a key must never land in a log or transcript.
impl std::fmt::Debug for ContentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContentKey(***)")
    }
}

/// Why an encryption/decryption/unwrap operation failed — all map to "cannot read"
/// (a tampered, wrong-key, or malformed blob), surfaced rather than silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// AEAD authentication failed (wrong key or tampered ciphertext/tag).
    Decrypt,
    /// The blob is too short / structurally malformed (truncated nonce, missing
    /// ephemeral key, wrong wrapped-key length).
    Malformed,
    /// A public key did not decode to a valid curve point.
    BadKey,
}

/// Encrypt `plaintext` under `key`. Output = `nonce(24) ‖ ciphertext+tag`. A **fresh
/// random nonce per call** (XChaCha20's 24-byte nonce makes random nonces safe), so
/// encrypting the same plaintext twice yields distinct ciphertexts.
pub fn encrypt(key: &ContentKey, plaintext: &[u8]) -> Vec<u8> {
    let cipher =
        XChaCha20Poly1305::new_from_slice(&key.0).expect("ContentKey is exactly KEY_LEN bytes");
    let nonce = rand::random::<[u8; NONCE_LEN]>();
    let ciphertext = cipher
        .encrypt(&XNonce::from(nonce), plaintext)
        .expect("XChaCha20-Poly1305 encryption is infallible for valid keys");
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    out
}

/// Decrypt a `nonce ‖ ciphertext` blob produced by [`encrypt`]. A wrong key or any
/// tamper fails the Poly1305 tag ⇒ [`CryptoError::Decrypt`].
pub fn decrypt(key: &ContentKey, blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if blob.len() < NONCE_LEN {
        return Err(CryptoError::Malformed);
    }
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    let cipher =
        XChaCha20Poly1305::new_from_slice(&key.0).expect("ContentKey is exactly KEY_LEN bytes");
    // `blob.len() >= NONCE_LEN` is checked above, so this slice is exactly NONCE_LEN bytes.
    // Convert slice → fixed array (std) → XNonce::from(array): works across the
    // chacha20poly1305 GenericArray→hybrid_array transition (avoids the deprecated from_slice).
    let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| CryptoError::Malformed)?;
    cipher
        .decrypt(&XNonce::from(nonce), ciphertext)
        .map_err(|_| CryptoError::Decrypt)
}

/// Wrap the content key `k` to a member identified by their **published X25519 wrap key**
/// (ADR-037 §D2 + ADR-041). Sealed-box form: a fresh ephemeral X25519 key does ECDH with
/// the recipient's wrap key, the shared secret is run through a SHA-256 KDF
/// (domain-separated + bound to both public keys), and `k` is AEAD-sealed under it.
/// Output = `ephemeral_x25519_pub(32) ‖ encrypt(wrap_key, k)`. Only the recipient's wrap
/// secret (derived from its Ed25519 seed via [`wrap_public_for`]'s twin) can unwrap.
pub fn wrap_to_member(
    k: &ContentKey,
    recipient_wrap_pub: &[u8; 32],
) -> Result<Vec<u8>, CryptoError> {
    // ADR-041 (#158 I1): `recipient_wrap_pub` is the member's PUBLISHED X25519 wrap key
    // (`wrap_public_for`), NOT their Ed25519 identity mapped to X25519 — signing and
    // key-exchange now use SEPARATE keys. The recipient must publish this key (a sender
    // cannot derive it from the Ed25519 public key alone).
    let recipient_x = XPublicKey::from(*recipient_wrap_pub);
    let ephemeral = StaticSecret::from(rand::random::<[u8; 32]>());
    let ephemeral_pub = XPublicKey::from(&ephemeral);
    let shared = ephemeral.diffie_hellman(&recipient_x);
    // F6 (security review): reject a non-contributory (all-zero / low-order) DH result so
    // a low-order recipient key can't force a predictable wrap key.
    if !shared.was_contributory() {
        return Err(CryptoError::Malformed);
    }
    let wrap_key = derive_wrap_key(
        shared.as_bytes(),
        ephemeral_pub.as_bytes(),
        recipient_wrap_pub,
    );
    let sealed = encrypt(&wrap_key, k.as_bytes());
    let mut out = Vec::with_capacity(X25519_LEN + sealed.len());
    out.extend_from_slice(ephemeral_pub.as_bytes());
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Unwrap a content key wrapped to my published wrap key by [`wrap_to_member`]. I derive
/// my **X25519 wrap secret** from my Ed25519 seed (the domain-separated subkey, ADR-041),
/// redo the ECDH against the blob's ephemeral public key, re-derive the wrap key, and
/// AEAD-open `k`. A blob wrapped to anyone else fails the tag ⇒ [`CryptoError::Decrypt`].
pub fn unwrap_as_member(
    blob: &[u8],
    my_ed25519_secret: &[u8; 32],
) -> Result<ContentKey, CryptoError> {
    if blob.len() < X25519_LEN {
        return Err(CryptoError::Malformed);
    }
    let (ephemeral_pub_bytes, sealed) = blob.split_at(X25519_LEN);
    let ephemeral_pub = XPublicKey::from(
        <[u8; 32]>::try_from(ephemeral_pub_bytes).map_err(|_| CryptoError::Malformed)?,
    );
    let my_x = derive_x25519_wrap_secret(my_ed25519_secret);
    let my_x_pub = XPublicKey::from(&my_x);
    let shared = my_x.diffie_hellman(&ephemeral_pub);
    // F6 (security review): the blob's `ephemeral_pub` is fully attacker-controlled — a
    // low-order point would force a known shared secret (hence a known wrap key). Reject a
    // non-contributory DH so an attacker can't seal an attacker-known content key to us.
    if !shared.was_contributory() {
        return Err(CryptoError::Malformed);
    }
    let wrap_key = derive_wrap_key(shared.as_bytes(), ephemeral_pub_bytes, my_x_pub.as_bytes());
    let mut k_bytes = decrypt(&wrap_key, sealed)?;
    let mut k: [u8; 32] = k_bytes
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::Malformed)?;
    let key = ContentKey::from_bytes(k);
    // #156 F9: wipe the raw key material copies (the Vec from `decrypt` + the array)
    // now that it lives inside the zeroizing `ContentKey`.
    k_bytes.zeroize();
    k.zeroize();
    Ok(key)
}

/// Derive the one-time AEAD wrap key from the ECDH shared secret. SHA-256 over a
/// domain-separation tag ‖ the shared secret ‖ both public keys — single-key
/// derivation from a high-entropy DH secret (NaCl-box style), avoiding an `hkdf`
/// dependency (which would pull a second `digest` major — see Cargo.toml note). The
/// public keys are bound in so the key is specific to this wrap.
fn derive_wrap_key(shared: &[u8], ephemeral_pub: &[u8], recipient_pub: &[u8]) -> ContentKey {
    let mut h = Sha256::new();
    h.update(b"mae-content-key-wrap/v1");
    h.update(shared);
    h.update(ephemeral_pub);
    h.update(recipient_pub);
    ContentKey::from_bytes(h.finalize().into())
}

/// ADR-041 (#158 I1): derive an identity's **dedicated X25519 content-key-wrap secret**
/// from its Ed25519 seed — `X25519_scalar = SHA-512("mae-x25519-wrap/v1" ‖ seed)[..32]`.
/// The domain-separation tag is what makes this key **distinct** from the signing key's
/// X25519 image (the old `crypto_sign_ed25519_sk_to_curve25519` map, tagless): signing
/// and key-exchange no longer share a key. One seed still backs everything (no extra
/// secret at rest); this is a deterministic, domain-separated subkey. The matching public
/// key is published per [`wrap_public_for`].
fn derive_x25519_wrap_secret(ed_secret_seed: &[u8; 32]) -> StaticSecret {
    let mut h = Sha512::new();
    h.update(b"mae-x25519-wrap/v1");
    h.update(ed_secret_seed);
    let mut hash = h.finalize();
    let mut x_secret = [0u8; 32];
    x_secret.copy_from_slice(&hash[..32]);
    let secret = StaticSecret::from(x_secret);
    // #156 F9: the SHA-512 expansion and the extracted scalar are the X25519 private key —
    // wipe both once `StaticSecret` (itself zeroizing on drop) holds a copy.
    x_secret.zeroize();
    hash.zeroize();
    secret
}

/// ADR-041 (#158 I1): the **published** X25519 wrap *public* key for an identity, derived
/// from its Ed25519 seed. A member publishes this in the signed membership op-log so the
/// owner can `wrap_to_member` the content key to it — the sender cannot derive it from the
/// member's Ed25519 *public* key (that's the whole point of the separation). Pure +
/// deterministic.
pub fn wrap_public_for(ed_secret_seed: &[u8; 32]) -> [u8; 32] {
    XPublicKey::from(&derive_x25519_wrap_secret(ed_secret_seed)).to_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    /// A table of independently-generated identities, returned as `(seed, ed_pub)`,
    /// so every property is checked across MANY keys — never a single magic seed.
    fn identities(n: usize) -> Vec<([u8; 32], [u8; 32])> {
        (0..n)
            .map(|i| {
                // Distinct, non-trivial seeds (not all-`i` — vary every byte).
                let mut seed = [0u8; 32];
                for (j, b) in seed.iter_mut().enumerate() {
                    *b = ((i as u32 * 131 + j as u32 * 7 + 17) % 251) as u8;
                }
                let pubk = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
                (seed, pubk)
            })
            .collect()
    }

    /// Varied plaintexts: empty, 1 byte, NUL-bearing/binary, and large (>64 KiB).
    fn plaintexts() -> Vec<Vec<u8>> {
        vec![
            vec![],
            vec![0u8],
            b"\x00\x01\x02 yrs-update with NUL".to_vec(),
            (0..70_000u32).map(|i| (i % 256) as u8).collect(),
        ]
    }

    /// #156 F9: the content key is wiped, not left in memory. `ZeroizeOnDrop` is a
    /// compile-time guarantee (the static assertion); here we also exercise the `Zeroize`
    /// impl directly — after `zeroize()` the bytes are all-zero, not the original key.
    #[test]
    fn content_key_is_zeroized() {
        use zeroize::Zeroize;
        let mut k = ContentKey::from_bytes([0x42u8; 32]);
        assert_eq!(k.as_bytes(), &[0x42u8; 32]);
        k.zeroize();
        assert_eq!(k.as_bytes(), &[0u8; 32], "Zeroize wipes the key bytes");

        fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
        assert_zeroize_on_drop::<ContentKey>();
    }

    #[test]
    fn aead_roundtrips_over_varied_inputs_and_keys() {
        for _ in 0..4 {
            let key = ContentKey::generate(); // a distinct random key each iteration
            for pt in plaintexts() {
                let blob = encrypt(&key, &pt);
                assert_eq!(decrypt(&key, &blob).unwrap(), pt, "roundtrip");
            }
        }
    }

    #[test]
    fn aead_uses_a_fresh_nonce_each_call() {
        let key = ContentKey::generate();
        let pt = b"same plaintext, same key";
        let a = encrypt(&key, pt);
        let b = encrypt(&key, pt);
        assert_ne!(
            a, b,
            "a fixed nonce would be catastrophic — ciphertexts must differ"
        );
        assert_eq!(decrypt(&key, &a).unwrap(), pt);
        assert_eq!(decrypt(&key, &b).unwrap(), pt, "both still decrypt");
    }

    #[test]
    fn aead_rejects_wrong_key_and_every_tampered_byte() {
        let key = ContentKey::generate();
        let other = ContentKey::generate();
        let blob = encrypt(&key, b"secret node body");
        assert_eq!(
            decrypt(&other, &blob),
            Err(CryptoError::Decrypt),
            "wrong key"
        );
        // Flip each byte position class: nonce region + ciphertext/tag region.
        for &pos in &[0usize, NONCE_LEN, blob.len() - 1] {
            let mut t = blob.clone();
            t[pos] ^= 0xff;
            assert_eq!(
                decrypt(&key, &t),
                Err(CryptoError::Decrypt),
                "tamper @ {pos}"
            );
        }
        assert_eq!(
            decrypt(&key, &blob[..NONCE_LEN - 1]),
            Err(CryptoError::Malformed)
        );
    }

    #[test]
    fn wrap_key_is_separate_from_the_signing_key_for_all_identities() {
        // ADR-041 (#158 I1): the published X25519 wrap key must be DISTINCT from the
        // identity's Ed25519 signing key AND from that key's birational X25519 image (the
        // thing we replaced) — signing and key-exchange use separate keys. It must be
        // deterministic and actually wrap. Checked across MANY identities, not one seed.
        use curve25519_dalek::edwards::CompressedEdwardsY;
        for (seed, ed_pub) in identities(16) {
            let wrap_pub = wrap_public_for(&seed);
            assert_ne!(
                wrap_pub, ed_pub,
                "wrap key must differ from the signing key"
            );
            let birational = CompressedEdwardsY(ed_pub)
                .decompress()
                .unwrap()
                .to_montgomery()
                .to_bytes();
            assert_ne!(
                wrap_pub, birational,
                "wrap key must differ from the ed25519->x25519 map (the domain tag separates them)"
            );
            assert_eq!(
                wrap_pub,
                wrap_public_for(&seed),
                "derivation is deterministic"
            );
            // It actually wraps: k → wrap-to-published-key → unwrap-with-seed.
            let k = ContentKey::generate();
            let blob = wrap_to_member(&k, &wrap_pub).unwrap();
            assert_eq!(unwrap_as_member(&blob, &seed).unwrap(), k);
        }
    }

    #[test]
    fn key_wrap_roundtrips_and_excludes_every_other_member() {
        let ids = identities(8);
        let k = ContentKey::generate();
        for (i, (seed_i, _pub_i)) in ids.iter().enumerate() {
            let blob = wrap_to_member(&k, &wrap_public_for(seed_i)).unwrap();
            // The intended recipient recovers k exactly.
            assert_eq!(
                unwrap_as_member(&blob, seed_i).unwrap(),
                k,
                "recipient {i} recovers k"
            );
            // EVERY other identity's secret fails — not just one "bob".
            for (j, (seed_j, _)) in ids.iter().enumerate() {
                if i != j {
                    assert_eq!(
                        unwrap_as_member(&blob, seed_j),
                        Err(CryptoError::Decrypt),
                        "non-recipient {j} must not unwrap {i}'s blob"
                    );
                }
            }
        }
    }

    #[test]
    fn key_wrap_rejects_tampering_in_every_segment() {
        let (seed, _pubk) = identities(1)[0];
        let k = ContentKey::generate();
        let blob = wrap_to_member(&k, &wrap_public_for(&seed)).unwrap();
        // ephemeral-pubkey region, nonce region, and ciphertext/tag region.
        for &pos in &[0usize, X25519_LEN, X25519_LEN + NONCE_LEN, blob.len() - 1] {
            let mut t = blob.clone();
            t[pos] ^= 0xff;
            assert!(
                unwrap_as_member(&t, &seed).is_err(),
                "tampered wrap @ {pos} must not unwrap"
            );
        }
        assert_eq!(
            unwrap_as_member(&blob[..X25519_LEN - 1], &seed),
            Err(CryptoError::Malformed),
            "truncated blob"
        );
    }

    #[test]
    fn distinct_keys_wrap_to_distinct_blobs() {
        // A fresh ephemeral per wrap ⇒ wrapping the SAME k to the SAME member twice
        // yields different blobs that both unwrap (no static ephemeral reuse).
        let (seed, _pubk) = identities(1)[0];
        let k = ContentKey::generate();
        let wrap_pub = wrap_public_for(&seed);
        let a = wrap_to_member(&k, &wrap_pub).unwrap();
        let b = wrap_to_member(&k, &wrap_pub).unwrap();
        assert_ne!(a, b, "ephemeral key must be fresh per wrap");
        assert_eq!(unwrap_as_member(&a, &seed).unwrap(), k);
        assert_eq!(unwrap_as_member(&b, &seed).unwrap(), k);
    }

    #[test]
    fn key_wrap_rejects_low_order_ephemeral_point() {
        // F6 (security review): a blob whose ephemeral pubkey is a low-order point forces
        // an all-zero shared secret — a wrap key the attacker knows. `unwrap` MUST reject
        // it (non-contributory DH) rather than open an attacker-chosen content key. The
        // all-zero u-coordinate is the canonical small-order X25519 point.
        let (seed, _pubk) = identities(1)[0];
        let mut hostile = vec![0u8; X25519_LEN]; // low-order ephemeral_pub
        hostile.extend_from_slice(&[7u8; NONCE_LEN + 16 + 32]); // arbitrary sealed region
        assert_eq!(
            unwrap_as_member(&hostile, &seed),
            Err(CryptoError::Malformed),
            "a low-order ephemeral point must be rejected before deriving the wrap key"
        );
        // Control: a legitimate wrap to the SAME member still round-trips (the check is
        // selective — it rejects only the non-contributory case).
        let k = ContentKey::generate();
        let ok = wrap_to_member(&k, &wrap_public_for(&seed)).unwrap();
        assert_eq!(unwrap_as_member(&ok, &seed).unwrap(), k);
    }
}
