//! MCP (Model Context Protocol) JSON-RPC types.
//!
//! @ai-caution: Sync message types are handled by `sync_exec.rs`.
//! Awareness types (`AwarenessState`) are implemented in `shared/sync/src/awareness.rs`,
//! wired through `daemon/src/collab_handler/sync_methods.rs::handle_sync_awareness`.
//! The existing message types remain stable — sync methods are additive.

use serde::{Deserialize, Serialize};

/// MCP protocol version — latest version we advertise.
pub const PROTOCOL_VERSION: &str = "2025-11-25";

/// All protocol versions this server accepts from clients.
/// Per spec, if the client requests a version we support, we MUST echo it back.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Given a client-requested version, return the version to echo back.
/// If the client's version is in our supported list, echo it. Otherwise return our latest.
pub fn negotiate_version(client_version: &str) -> &'static str {
    for &v in SUPPORTED_VERSIONS {
        if v == client_version {
            return v;
        }
    }
    PROTOCOL_VERSION
}

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpError>,
}

impl JsonRpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: serde_json::Value, error: McpError) -> Self {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpError {
    pub code: i64,
    pub message: String,
}

impl McpError {
    pub fn parse_error(message: String) -> Self {
        McpError {
            code: -32700,
            message,
        }
    }

    pub fn method_not_found(message: String) -> Self {
        McpError {
            code: -32601,
            message,
        }
    }

    pub fn invalid_request(message: String) -> Self {
        McpError {
            code: -32600,
            message,
        }
    }

    pub fn internal_error(message: String) -> Self {
        McpError {
            code: -32603,
            message,
        }
    }

    // Application-level error codes (MCP/JSON-RPC -32000 range)

    pub fn backpressure(message: String) -> Self {
        McpError {
            code: -32000,
            message,
        }
    }

    pub fn editor_busy(message: String) -> Self {
        McpError {
            code: -32001,
            message,
        }
    }

    pub fn tool_not_found(message: String) -> Self {
        McpError {
            code: -32002,
            message,
        }
    }

    pub fn invalid_session(message: String) -> Self {
        McpError {
            code: -32003,
            message,
        }
    }

    pub fn session_expired(message: String) -> Self {
        McpError {
            code: -32004,
            message,
        }
    }
}

/// MCP initialize result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: serde_json::Value,
    /// Optional server→client guidance surfaced to the model by compliant
    /// clients (part of the MCP spec's `initialize` response; previously
    /// unimplemented here — structurally absent, not just unset). Caller-
    /// supplied (e.g. `McpServer::with_instructions`) since this crate is
    /// intentionally transport-generic with no KB/editor knowledge of its
    /// own — see `mae_ai::guidance` for the content this typically carries
    /// (a designated guidance KB + registered KB names).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Server capabilities declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<serde_json::Value>,
}

/// Tool definition for MCP tools/list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    /// The tool's permission tier ("ReadOnly"/"Write"/"Shell"/"Privileged"),
    /// as a plain string so this crate doesn't need to depend on `mae-ai`'s
    /// `PermissionTier` type. `None` for a tool with no declared tier
    /// (callers should treat that the same as an unknown/untiered tool, not
    /// as evidence it's safe). Added after a real incident: without this,
    /// `tools/list` transmitted no tier information at all, so every
    /// external client (`mae-agent` included) silently treated every tool as
    /// the default `Write` tier regardless of its real tier -- meaning a
    /// Shell-tier tool like `shell_exec` was never distinguishable from a
    /// Write-tier one by any client-side permission gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    /// Standard MCP tool annotations (readOnlyHint et al.), which clients
    /// like VS Code's Copilot agent mode use to skip the confirmation
    /// dialog on safe reads. `None` when the caller has no declared
    /// permission tier for this tool to derive annotations from (ADR-050 D2
    /// -- callers must derive these mechanically from `PermissionTier`, never
    /// hand-author them per tool, to avoid drift across 700+ tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

/// MCP-standard tool annotations (see the MCP tool-annotations spec).
/// `title` is a display-friendly name distinct from `ToolInfo::name`; the
/// three `*_hint` fields are advisory hints, not security guarantees --
/// server-side enforcement (`PermissionPolicy`/`kb_access`) is the actual
/// boundary regardless of what a client does with these hints (see ADR-051).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// True if the tool never mutates editor/KB/filesystem state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    /// True if the tool may perform a destructive update (irreversible or
    /// hard to undo). Only meaningful when `read_only_hint` is false/absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    /// True if calling the tool repeatedly with the same arguments has no
    /// additional effect beyond the first call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
}

/// Result of a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    pub content: Vec<ContentItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// A content item in a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentItem {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_initialize_result() {
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities {
                tools: Some(serde_json::json!({})),
            },
            server_info: serde_json::json!({
                "name": "mae-editor",
                "version": "0.3.0",
            }),
            instructions: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("protocolVersion"));
        assert!(json.contains("2025-11-25"));
    }

    #[test]
    fn initialize_result_omits_instructions_field_when_none() {
        // Backward compat: older clients parsing this response must see no
        // instructions field at all when unset, not `"instructions":null`.
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities { tools: None },
            server_info: serde_json::json!({}),
            instructions: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("instructions"));
    }

    #[test]
    fn initialize_result_round_trips_instructions_when_present() {
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities { tools: None },
            server_info: serde_json::json!({}),
            instructions: Some("Consult KB 'dev-practices' first.".to_string()),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"instructions\":\"Consult KB 'dev-practices' first.\""));
        let round_tripped: InitializeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(
            round_tripped.instructions.as_deref(),
            Some("Consult KB 'dev-practices' first.")
        );
    }

    #[test]
    fn test_deserialize_tool_call() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "read_buffer",
                "arguments": {"buffer_index": 0}
            }
        }"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "tools/call");
        let params = req.params.unwrap();
        assert_eq!(params["name"], "read_buffer");
    }

    #[test]
    fn test_tool_definition_to_mcp_schema() {
        let tool = ToolInfo {
            name: "read_buffer".to_string(),
            description: "Read buffer contents".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "buffer_index": {"type": "integer"}
                }
            }),
            permission: None,
            annotations: None,
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["name"], "read_buffer");
        assert!(json["inputSchema"]["properties"]["buffer_index"].is_object());
    }

    #[test]
    fn negotiate_version_echoes_supported() {
        assert_eq!(negotiate_version("2025-11-25"), "2025-11-25");
        assert_eq!(negotiate_version("2024-11-05"), "2024-11-05");
        assert_eq!(negotiate_version("2025-06-18"), "2025-06-18");
        assert_eq!(negotiate_version("2025-03-26"), "2025-03-26");
    }

    #[test]
    fn negotiate_version_unknown_returns_latest() {
        assert_eq!(negotiate_version("9999-01-01"), PROTOCOL_VERSION);
        assert_eq!(negotiate_version("2023-01-01"), PROTOCOL_VERSION);
    }

    #[test]
    fn tool_info_omits_annotations_when_none() {
        let tool = ToolInfo {
            name: "kb_search".to_string(),
            description: "Search the KB".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            permission: None,
            annotations: None,
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(!json.contains("annotations"));
    }

    #[test]
    fn tool_info_serializes_annotations_as_camel_case() {
        let tool = ToolInfo {
            name: "kb_search".to_string(),
            description: "Search the KB".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            permission: Some("ReadOnly".to_string()),
            annotations: Some(ToolAnnotations {
                title: None,
                read_only_hint: Some(true),
                destructive_hint: Some(false),
                idempotent_hint: Some(true),
            }),
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["annotations"]["readOnlyHint"], true);
        assert_eq!(json["annotations"]["destructiveHint"], false);
        assert_eq!(json["annotations"]["idempotentHint"], true);
        // `title` was None -- must be structurally absent, not `null`, so a
        // strict client doesn't choke on an unexpected null string field.
        assert!(!json["annotations"]
            .as_object()
            .unwrap()
            .contains_key("title"));
    }
}
