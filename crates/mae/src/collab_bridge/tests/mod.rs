// Test modules split from monolithic collab_bridge_tests.rs (4,497 lines, 126 tests).

pub(crate) use super::*;

mod collab_bridge_buffer_join_tests;
mod collab_bridge_e2e_derive_encrypt_tests;
mod collab_bridge_e2e_flags_offline_tests;
mod collab_bridge_e2e_recovery_tests;
mod collab_bridge_e2e_rotation_tests;
mod collab_bridge_fence_conflict_tests;
mod collab_bridge_identity_tofu_tests;
mod collab_bridge_join_save_tests;
mod collab_bridge_kb_crypto_tests;
mod collab_bridge_kb_sync_tests;
mod collab_bridge_message_handling_tests;
mod collab_bridge_psk_tests;
mod collab_bridge_sync_recovery_backoff_tests;

// Shared test helpers/macros used across multiple test modules

/// ADR-037 §2b: a fresh, empty content-encryption context for the message-handler
/// tests that don't exercise encryption (the temporaries live for the call). Tests
/// that DO exercise encryption build a `KbCryptoCtx` explicitly with real state.
macro_rules! kb_ctx {
    () => {
        &mut KbCryptoCtx {
            content_keys: &mut std::collections::HashMap::new(),
            op_sets: &mut std::collections::HashMap::new(),
            node_to_kb: &mut std::collections::HashMap::new(),
            seen_ops: &mut std::collections::HashMap::new(),
            kb_collections: &mut std::collections::HashMap::new(),
            signing_identity: None,
            pending_collection_ops: &mut Vec::new(),
        }
    };
}
pub(crate) use kb_ctx;
