//! A KB's history key: what a version's `content_hash` is keyed with
//! (ADR-116 D3, as amended).
//!
//! An unkeyed hash of the content would let anyone holding the store confirm a
//! guess at a version's whole content, and so brute-force a small secret in a
//! templated record. Keyed, the hash confirms nothing without the key. While a
//! KB is plaintext the daemon holds its key, which exposes nothing new: it
//! holds the plaintext. When the KB switches to E2E, the key goes to the owner
//! with the content key and the daemon releases its copy, so the stored hashes
//! stop being an oracle without the chain being rewritten (Amendment 1).

use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::{encode_content, Hash, MaterializedNode};

/// A KB's 256-bit history key.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct HistoryKey([u8; 32]);

impl std::fmt::Debug for HistoryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HistoryKey(…)")
    }
}

impl HistoryKey {
    /// A fresh random key.
    pub fn generate() -> Self {
        HistoryKey(rand::random::<[u8; 32]>())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        HistoryKey(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The keyed hash of `node`: HMAC-SHA256 over its canonical encoding,
    /// under a domain of its own.
    pub fn content_hash(&self, node: &MaterializedNode) -> Hash {
        let mut msg = b"maehistory-content/v1".to_vec();
        msg.extend(encode_content(node));
        hmac_sha256(&self.0, &msg)
    }
}

/// HMAC-SHA256 (RFC 2104). Written out rather than pulled in: it is a few
/// lines over the `sha2` already here, and another `digest`-family crate would
/// risk a second `digest` major (see the crate's Cargo.toml). Pinned to the RFC
/// 4231 test vectors.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> Hash {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let (mut ipad, mut opad) = ([0x36u8; BLOCK], [0x5cu8; BLOCK]);
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    k.zeroize();
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}
