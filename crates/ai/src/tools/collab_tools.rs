use crate::types::*;

use super::tool_def::ToolDefBuilder;

/// Collaborative editing tool definitions.
pub(super) fn collab_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefBuilder::new(
            "collab_status",
            "Return the current collaborative editing status: connection state, peer count, synced document count, and server address.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "collab_connect",
            "Connect to a mae-daemon for collaborative editing. Queues a connection intent for the event loop.",
        )
        .prop(
            "address",
            "string",
            "Server address as host:port (default: 127.0.0.1:9473)",
        )
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "collab_share",
            "Share a buffer for collaborative editing via the connected mae-daemon. The buffer must exist.",
        )
        .prop("buffer", "string", "Name of the buffer to share")
        .required(["buffer"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "collab_doctor",
            "Run collaborative editing diagnostics. Checks connectivity, latency, and sync health. Results appear in the status buffer.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "collab_list",
            "List all shared documents on the connected mae-daemon. Returns doc names, sizes, and peer counts.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "collab_discover",
            "Discover MAE peers on the local network via mDNS. Returns discovered peer names, addresses, and shared KB counts.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "collab_rotate_identity",
            "Rotate this peer's collab identity key (ADR-040) across every KB it owns AND belongs to: cross-signs a successor key and the owner re-wraps E2e content keys. After it ships, the new key must be authorized on the daemon out-of-band, then reconnect. Requires `key` auth mode + an active connection.",
        )
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "collab_register_recovery_key",
            "Register a fresh OFFLINE recovery key (ADR-040 §Recovery-key) across every KB this peer belongs to, so a future key loss is recoverable. The recovery secret is saved locally and MUST be backed up offline (whoever holds it can rotate your identity). Latest registration wins.",
        )
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "collab_recover_identity",
            "Recover a lost/compromised primary key using a pre-registered offline recovery key (ADR-040 §Recovery-key). Run AS the new key (already authorized + connected out-of-band): authors a recovery-signed rebind so the new key inherits the lost key's KB seats.",
        )
        .prop(
            "recovery_path",
            "string",
            "Directory holding the restored offline recovery key (an `id_ed25519` file)",
        )
        .prop(
            "old_fingerprint",
            "string",
            "The lost key's fingerprint (SHA256:…) to rotate onto the new key",
        )
        .required(["recovery_path", "old_fingerprint"])
        .permission(PermissionTier::Write)
        .build(),
    ]
}
