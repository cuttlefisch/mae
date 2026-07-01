use std::collections::HashMap;

use crate::types::*;

/// Collaborative editing tool definitions.
pub(super) fn collab_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "collab_status".into(),
            description: "Return the current collaborative editing status: connection state, peer count, synced document count, and server address.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "collab_connect".into(),
            description: "Connect to a mae-daemon for collaborative editing. Queues a connection intent for the event loop.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "address".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Server address as host:port (default: 127.0.0.1:9473)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "collab_share".into(),
            description: "Share a buffer for collaborative editing via the connected mae-daemon. The buffer must exist.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "buffer".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Name of the buffer to share".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["buffer".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "collab_doctor".into(),
            description: "Run collaborative editing diagnostics. Checks connectivity, latency, and sync health. Results appear in the status buffer.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "collab_list".into(),
            description: "List all shared documents on the connected mae-daemon. Returns doc names, sizes, and peer counts.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "collab_discover".into(),
            description: "Discover MAE peers on the local network via mDNS. Returns discovered peer names, addresses, and shared KB counts.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "collab_rotate_identity".into(),
            description: "Rotate this peer's collab identity key (ADR-040) across every KB it owns AND belongs to: cross-signs a successor key and the owner re-wraps E2e content keys. After it ships, the new key must be authorized on the daemon out-of-band, then reconnect. Requires `key` auth mode + an active connection.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "collab_register_recovery_key".into(),
            description: "Register a fresh OFFLINE recovery key (ADR-040 §Recovery-key) across every KB this peer belongs to, so a future key loss is recoverable. The recovery secret is saved locally and MUST be backed up offline (whoever holds it can rotate your identity). Latest registration wins.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "collab_recover_identity".into(),
            description: "Recover a lost/compromised primary key using a pre-registered offline recovery key (ADR-040 §Recovery-key). Run AS the new key (already authorized + connected out-of-band): authors a recovery-signed rebind so the new key inherits the lost key's KB seats.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "recovery_path".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Directory holding the restored offline recovery key (an `id_ed25519` file)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "old_fingerprint".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "The lost key's fingerprint (SHA256:…) to rotate onto the new key".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["recovery_path".into(), "old_fingerprint".into()],
            },
            permission: Some(PermissionTier::Write),
        },
    ]
}
