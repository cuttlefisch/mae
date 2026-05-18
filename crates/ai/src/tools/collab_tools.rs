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
            description: "Connect to a mae-state-server for collaborative editing. Queues a connection intent for the event loop.".into(),
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
            description: "Share a buffer for collaborative editing via the connected state server. The buffer must exist.".into(),
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
    ]
}
