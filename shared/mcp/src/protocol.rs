//! MCP (Model Context Protocol) JSON-RPC types.
//!
//! @ai-caution: [dispatch] Sync message types are handled by `sync_exec.rs`.
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
    /// JSON-RPC's optional `data` member. When present it carries `kind`, a
    /// stable string a client branches on (never the message text). Boxed so
    /// an error stays small on the paths that return one by value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<serde_json::Value>>,
}

impl McpError {
    pub fn parse_error(message: String) -> Self {
        McpError {
            code: -32700,
            message,
            data: None,
        }
    }

    pub fn method_not_found(message: String) -> Self {
        McpError {
            code: -32601,
            message,
            data: None,
        }
    }

    pub fn invalid_request(message: String) -> Self {
        McpError {
            code: -32600,
            message,
            data: None,
        }
    }

    pub fn internal_error(message: String) -> Self {
        McpError {
            code: -32603,
            message,
            data: None,
        }
    }

    // Application-level error codes (MCP/JSON-RPC -32000 range)

    pub fn backpressure(message: String) -> Self {
        McpError {
            code: -32000,
            message,
            data: None,
        }
    }

    pub fn editor_busy(message: String) -> Self {
        McpError {
            code: -32001,
            message,
            data: None,
        }
    }

    pub fn tool_not_found(message: String) -> Self {
        McpError {
            code: -32002,
            message,
            data: None,
        }
    }

    pub fn invalid_session(message: String) -> Self {
        McpError {
            code: -32003,
            message,
            data: None,
        }
    }

    pub fn session_expired(message: String) -> Self {
        McpError {
            code: -32004,
            message,
            data: None,
        }
    }

    /// The requested KB id already belongs to a different owner (ADR-105 D5).
    ///
    /// A distinct code rather than a generic `invalid_request`, because the client
    /// must be able to *act* on it: an id that was minted but never confirmed is
    /// safe to discard and re-mint, and that recovery cannot be driven by
    /// string-matching an error message. See `KB_ID_OWNED_BY_ANOTHER`.
    pub fn kb_id_owned_by_another(message: String) -> Self {
        Self::kinded(
            KB_ID_OWNED_BY_ANOTHER,
            message,
            "kb_id_owned_by_another",
            serde_json::json!({}),
        )
    }
}

impl McpError {
    /// JSON-RPC's -32602: a parameter is missing, has the wrong type or is out
    /// of range. `data` is `{"kind": "invalid_params", "param": <name>}`, so a
    /// client learns which parameter without parsing the message.
    pub fn invalid_params(param: &str, message: String) -> Self {
        McpError {
            code: INVALID_PARAMS,
            message,
            data: Some(Box::new(serde_json::json!({
                "kind": "invalid_params",
                "param": param,
            }))),
        }
    }

    /// An update would grow a document past the daemon's size limit (ADR-109
    /// D6). Nothing was applied, logged or relayed. `data` is
    /// `{"kind": "quota_exceeded", "size": <bytes it would hold>, "limit": <bytes>}`.
    pub fn document_too_large(message: String, size: usize, limit: usize) -> Self {
        McpError {
            code: DOCUMENT_TOO_LARGE,
            message,
            data: Some(Box::new(serde_json::json!({
                "kind": "quota_exceeded",
                "size": size,
                "limit": limit,
            }))),
        }
    }
}

/// Refusal constructors for the daemon's KB surfaces: the collab listener's
/// `kb/*`, `sync/*` and `docs/*` methods and `kb/query.*`.
///
/// Each one fixes `data.kind` — the stable snake_case string a client branches
/// on — and the extra `data` fields that kind carries. A client must never parse
/// the message text; it is for humans and logs. The JSON-RPC code is the one
/// the refusal already carried before kinds existed (mostly `-32603`), kept so
/// a client keyed on codes sees no change. The table operators and client
/// authors read is in `docs/DAEMON_ADMIN.md` ("Error kinds"); [`ERROR_KINDS`]
/// lists every kind, and a daemon test holds that table to it.
impl McpError {
    /// The one place a kinded error is built: `extra` must be a JSON object
    /// and is merged next to `kind`.
    fn kinded(code: i64, message: String, kind: &str, extra: serde_json::Value) -> Self {
        let mut data = match extra {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        data.insert("kind".to_string(), serde_json::Value::from(kind));
        McpError {
            code,
            message,
            data: Some(Box::new(serde_json::Value::Object(data))),
        }
    }

    /// The caller is not a member of the KB. Also what a non-member gets for
    /// ANY question about the KB's content, so it never learns whether a node
    /// exists (see `node_not_in_kb`).
    pub fn not_member(message: String) -> Self {
        Self::kinded(-32603, message, "not_member", serde_json::json!({}))
    }

    /// The caller is a member, but its role does not permit the operation.
    /// `required` (`"editor"` or `"owner"`) when the gate knows it.
    pub fn role_forbidden(message: String, required: Option<&str>) -> Self {
        let extra = match required {
            Some(r) => serde_json::json!({ "required": r }),
            None => serde_json::json!({}),
        };
        Self::kinded(-32603, message, "role_forbidden", extra)
    }

    /// The KB is not shared over the transport the request arrived on
    /// (`"hub"` or `"p2p"`).
    pub fn transport_forbidden(message: String, transport: &str) -> Self {
        Self::kinded(
            -32603,
            message,
            "transport_forbidden",
            serde_json::json!({ "transport": transport }),
        )
    }

    /// A member restricted to live queries (ADR-067) asked to replicate the KB.
    pub fn replication_restricted(message: String) -> Self {
        Self::kinded(
            -32603,
            message,
            "replication_restricted",
            serde_json::json!({}),
        )
    }

    /// No KB with this id is hosted here.
    pub fn kb_not_found(message: String) -> Self {
        Self::kinded(-32603, message, "kb_not_found", serde_json::json!({}))
    }

    /// The KB's collection document exists but could not be read or decoded.
    pub fn collection_unavailable(message: String) -> Self {
        Self::kinded(
            -32603,
            message,
            "collection_unavailable",
            serde_json::json!({}),
        )
    }

    /// The caller may read the KB, and the KB's manifest does not list this
    /// node. Only ever returned after the read gate passed.
    pub fn node_not_in_kb(message: String) -> Self {
        Self::kinded(-32603, message, "node_not_in_kb", serde_json::json!({}))
    }

    /// A write carries an op from a stale authorization epoch (ADR-023): adopt
    /// the authoritative state and re-author under `current_epoch`.
    /// `caller_epoch` is the epoch the op claimed, when the op names one (a
    /// signed content op does; a bare CRDT update does not).
    pub fn epoch_fenced(message: String, current_epoch: u64, caller_epoch: Option<u64>) -> Self {
        let mut extra = serde_json::json!({ "current_epoch": current_epoch });
        if let Some(e) = caller_epoch {
            extra["caller_epoch"] = serde_json::json!(e);
        }
        Self::kinded(-32603, message, "epoch_fenced", extra)
    }

    /// A signed op's signature, or the header binding it, does not verify.
    pub fn signature_invalid(message: String) -> Self {
        Self::kinded(-32603, message, "signature_invalid", serde_json::json!({}))
    }

    /// An unsigned content op on a path that requires signatures (the mesh).
    pub fn signature_required(message: String) -> Self {
        Self::kinded(-32603, message, "signature_required", serde_json::json!({}))
    }

    /// A rebirth (ADR-107) that is not bound to an owner-signed `Rebirth` op.
    pub fn rebirth_unverified(message: String) -> Self {
        Self::kinded(-32603, message, "rebirth_unverified", serde_json::json!({}))
    }

    /// A KB document named on a path that cannot authorize one. Deliberately
    /// says nothing about whether the document exists.
    pub fn doc_unavailable(message: String) -> Self {
        Self::kinded(-32603, message, "doc_unavailable", serde_json::json!({}))
    }

    /// A KB node document whose address names a different KB from the
    /// request's `kb_id`. Every check on the request runs against `kb_id`, so
    /// the write is refused rather than authorized against the wrong KB.
    pub fn doc_kb_mismatch(message: String) -> Self {
        Self::kinded(-32603, message, "doc_kb_mismatch", serde_json::json!({}))
    }

    /// A daemon-local administrative method (the local blocklist) called by
    /// a principal that is not this daemon's operator.
    pub fn not_operator(message: String) -> Self {
        Self::kinded(-32603, message, "not_operator", serde_json::json!({}))
    }

    /// A document that must already exist does not.
    pub fn doc_not_found(message: String) -> Self {
        Self::kinded(-32603, message, "doc_not_found", serde_json::json!({}))
    }

    /// The caller's tenant is over its request quota or connection cap.
    pub fn tenant_quota_exceeded(message: String) -> Self {
        Self::kinded(
            -32603,
            message,
            "tenant_quota_exceeded",
            serde_json::json!({}),
        )
    }

    /// One update is larger than `[collab.sync] max_update_size_bytes`.
    /// Distinct from `quota_exceeded`, which is about the document it would grow.
    pub fn update_too_large(message: String, size: usize, limit: usize) -> Self {
        Self::kinded(
            -32603,
            message,
            "update_too_large",
            serde_json::json!({ "size": size, "limit": limit }),
        )
    }

    /// `kb/remove_member` named the KB's owner.
    pub fn cannot_remove_owner(message: String) -> Self {
        Self::kinded(
            -32603,
            message,
            "cannot_remove_owner",
            serde_json::json!({}),
        )
    }

    /// This daemon cannot author the signed membership op the request needs:
    /// it has no signing identity, its identity is not the KB's owner, or the
    /// KB has no signed log to append to.
    pub fn signer_unavailable(message: String) -> Self {
        Self::kinded(-32603, message, "signer_unavailable", serde_json::json!({}))
    }

    /// `kb/revoke` named someone who is not a current member.
    pub fn subject_not_member(message: String) -> Self {
        Self::kinded(-32603, message, "subject_not_member", serde_json::json!({}))
    }

    /// The method needs an authenticated principal and the connection has none.
    pub fn not_authenticated(message: String) -> Self {
        Self::kinded(-32603, message, "not_authenticated", serde_json::json!({}))
    }

    /// `kb/query.self_token` on a daemon that does not issue tokens.
    pub fn self_issue_disabled(message: String) -> Self {
        Self::kinded(
            -32603,
            message,
            "self_issue_disabled",
            serde_json::json!({}),
        )
    }

    /// An E2E KB with no signed content anchor, so nothing can be verified
    /// against it.
    pub fn no_content_anchor(message: String) -> Self {
        Self::kinded(-32603, message, "no_content_anchor", serde_json::json!({}))
    }

    /// The HTTPS listener does not serve `kb/query.*` on this daemon (turned
    /// off, or there is no document store to serve it from).
    pub fn kb_query_unavailable(message: String) -> Self {
        Self::kinded(
            -32603,
            message,
            "kb_query_unavailable",
            serde_json::json!({}),
        )
    }

    /// A server-side operation the daemon cannot perform on an E2E KB,
    /// because it holds no key.
    pub fn e2e_unsupported(message: String) -> Self {
        Self::kinded(-32600, message, "e2e_unsupported", serde_json::json!({}))
    }

    /// The daemon failed (storage, decode, persistence). Not a decision about
    /// the caller; retrying may succeed.
    pub fn internal_fault(message: String) -> Self {
        Self::kinded(-32603, message, "internal", serde_json::json!({}))
    }
}

/// Every `data.kind` the daemon's KB surfaces return. Documented in
/// `docs/DAEMON_ADMIN.md`; a daemon test fails if that table and this list
/// disagree.
pub const ERROR_KINDS: &[&str] = &[
    "invalid_params",
    "quota_exceeded",
    "kb_id_owned_by_another",
    "not_member",
    "role_forbidden",
    "transport_forbidden",
    "replication_restricted",
    "kb_not_found",
    "collection_unavailable",
    "node_not_in_kb",
    "epoch_fenced",
    "signature_invalid",
    "signature_required",
    "rebirth_unverified",
    "doc_unavailable",
    "doc_kb_mismatch",
    "doc_not_found",
    "tenant_quota_exceeded",
    "update_too_large",
    "cannot_remove_owner",
    "signer_unavailable",
    "subject_not_member",
    "not_authenticated",
    "not_operator",
    "self_issue_disabled",
    "no_content_anchor",
    "kb_query_unavailable",
    "e2e_unsupported",
    "internal",
];

/// JSON-RPC 2.0's "Invalid params". See [`McpError::invalid_params`].
pub const INVALID_PARAMS: i64 = -32602;

/// An update was refused: it would grow a document past the daemon's size
/// limit (ADR-109 D6). See [`McpError::document_too_large`].
pub const DOCUMENT_TOO_LARGE: i64 = -32006;

/// `kb/share` refused: the id already belongs to a different owner (ADR-105 D5).
///
/// Named so both sides reference one constant — the daemon raises it and the
/// editor branches on it to decide whether re-minting is safe. A magic number
/// duplicated at those two sites is precisely the kind of drift that turns a
/// recovery path into a silent no-op.
pub const KB_ID_OWNED_BY_ANOTHER: i64 = -32005;

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

    /// `data` is new on the wire: absent when unset (an old client sees
    /// exactly what it saw before), and an error without it still parses.
    #[test]
    fn error_data_is_optional_on_the_wire() {
        let plain = serde_json::to_value(McpError::internal_error("x".into())).unwrap();
        assert_eq!(plain, serde_json::json!({"code": -32603, "message": "x"}));
        let old: McpError = serde_json::from_str(r#"{"code":-32603,"message":"x"}"#).unwrap();
        assert!(old.data.is_none());

        let refused =
            serde_json::to_value(McpError::document_too_large("big".into(), 11, 10)).unwrap();
        assert_eq!(refused["code"], DOCUMENT_TOO_LARGE);
        assert_eq!(refused["data"]["kind"], "quota_exceeded");
        assert_eq!(
            (
                refused["data"]["size"].as_u64(),
                refused["data"]["limit"].as_u64()
            ),
            (Some(11), Some(10))
        );
    }

    /// Every kinded constructor, so the test below can hold [`ERROR_KINDS`]
    /// to what is actually produced — in both directions.
    fn every_kinded_error() -> Vec<McpError> {
        let m = || "m".to_string();
        vec![
            McpError::invalid_params("p", m()),
            McpError::document_too_large(m(), 2, 1),
            McpError::kb_id_owned_by_another(m()),
            McpError::not_member(m()),
            McpError::role_forbidden(m(), Some("editor")),
            McpError::transport_forbidden(m(), "p2p"),
            McpError::replication_restricted(m()),
            McpError::kb_not_found(m()),
            McpError::collection_unavailable(m()),
            McpError::node_not_in_kb(m()),
            McpError::epoch_fenced(m(), 9, Some(3)),
            McpError::signature_invalid(m()),
            McpError::signature_required(m()),
            McpError::rebirth_unverified(m()),
            McpError::doc_unavailable(m()),
            McpError::doc_kb_mismatch(m()),
            McpError::doc_not_found(m()),
            McpError::tenant_quota_exceeded(m()),
            McpError::update_too_large(m(), 2, 1),
            McpError::cannot_remove_owner(m()),
            McpError::signer_unavailable(m()),
            McpError::subject_not_member(m()),
            McpError::not_authenticated(m()),
            McpError::not_operator(m()),
            McpError::self_issue_disabled(m()),
            McpError::no_content_anchor(m()),
            McpError::kb_query_unavailable(m()),
            McpError::e2e_unsupported(m()),
            McpError::internal_fault(m()),
        ]
    }

    #[test]
    fn error_kinds_are_exactly_the_kinds_the_constructors_produce() {
        let produced: Vec<String> = every_kinded_error()
            .iter()
            .map(|e| {
                let data = e.data.as_deref().expect("a kinded error carries data");
                data["kind"].as_str().expect("kind is a string").to_string()
            })
            .collect();
        let listed: Vec<String> = ERROR_KINDS.iter().map(|k| k.to_string()).collect();
        assert_eq!(
            produced, listed,
            "ERROR_KINDS must list every constructor, in order"
        );
        let unique: std::collections::BTreeSet<_> = listed.iter().collect();
        assert_eq!(unique.len(), listed.len(), "a kind is listed twice");
        for k in ERROR_KINDS {
            assert!(
                k.starts_with(|c: char| c.is_ascii_lowercase())
                    && k.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{k} is not snake_case"
            );
        }
    }

    /// Extra fields ride next to `kind`, and an unknown caller epoch is absent
    /// rather than a misleading `null` or `0`.
    #[test]
    fn kinded_errors_carry_their_extra_fields() {
        let fenced = McpError::epoch_fenced("f".into(), 7, None);
        let d = fenced.data.as_deref().unwrap();
        assert_eq!(d["current_epoch"], 7);
        assert!(d.get("caller_epoch").is_none(), "{d}");
        let fenced = McpError::epoch_fenced("f".into(), 7, Some(2));
        assert_eq!(fenced.data.as_deref().unwrap()["caller_epoch"], 2);

        let role = McpError::role_forbidden("r".into(), None);
        assert!(role.data.as_deref().unwrap().get("required").is_none());
        let owned = McpError::kb_id_owned_by_another("o".into());
        assert_eq!(
            owned.code, KB_ID_OWNED_BY_ANOTHER,
            "the code is what the editor keys on"
        );
        assert_eq!(owned.message, "o");
    }

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
