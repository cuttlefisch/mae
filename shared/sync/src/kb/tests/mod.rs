//! Tests for the `kb` submodules, grouped by the same theme split as the
//! source: one test file per module (`node`, `collection_core`,
//! `collection_roles`, `collection_oplog`, `collection_crypto`).

use super::*;

/// (secret seed, public key bytes, principal fingerprint) for a test identity.
/// Shared by the oplog and crypto test groups (both author signed ops).
pub(super) fn oplog_keypair(seed: u8) -> ([u8; 32], [u8; 32], String) {
    use crate::membership::fingerprint_of;
    use ed25519_dalek::SigningKey;
    let secret = [seed; 32];
    let pubkey = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
    let fp = fingerprint_of(&pubkey);
    (secret, pubkey, fp)
}

mod collection_core_tests;
mod collection_crypto_recovery_tests;
mod collection_crypto_rotation_tests;
mod collection_crypto_tests;
mod collection_oplog_tests;
mod collection_roles_tests;
mod node_tests;
