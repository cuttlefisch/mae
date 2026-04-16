//! LSP JSON-RPC 2.0 message types.
//!
//! LSP uses JSON-RPC 2.0 over Content-Length framed transport (same framing as DAP).
//! Messages come in three flavors: Request (has id), Notification (no id), and
//! Response (has id, result or error).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 base types
// ---------------------------------------------------------------------------

/// A JSON-RPC 2.0 message — request, notification, or response.
/// Order matters for `#[serde(untagged)]`: Request must come before Response
/// because both have `id`, but Request also has `method` which is required.
/// If Response were first, requests would deserialize as responses with
/// `result: None, error: None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

/// A JSON-RPC request (has `id` and `method`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// A JSON-RPC notification (has `method` but no `id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// A JSON-RPC response (has `id`, plus `result` or `error`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

/// Request ID — integer or string per JSON-RPC 2.0 spec.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Integer(i64),
    String(String),
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

impl Request {
    pub fn new(id: i64, method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Request {
            jsonrpc: "2.0".into(),
            id: RequestId::Integer(id),
            method: method.into(),
            params,
        }
    }
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Notification {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        }
    }
}

impl Response {
    pub fn ok(id: RequestId, result: serde_json::Value) -> Self {
        Response {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: RequestId, code: i64, message: impl Into<String>) -> Self {
        Response {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(ResponseError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// LSP-specific types — Initialize
// ---------------------------------------------------------------------------

/// Client capabilities sent during initialize.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_document: Option<TextDocumentClientCapabilities>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentClientCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synchronization: Option<TextDocumentSyncClientCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_diagnostics: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentSyncClientCapabilities {
    #[serde(default)]
    pub did_save: bool,
    #[serde(default)]
    pub will_save: bool,
    #[serde(default)]
    pub will_save_wait_until: bool,
    #[serde(default)]
    pub dynamic_registration: bool,
}

/// Initialize request params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub process_id: Option<i64>,
    pub root_uri: Option<String>,
    pub capabilities: ClientCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<ClientInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Server capabilities returned from initialize.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_document_sync: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_provider: Option<serde_json::Value>,
    #[serde(default)]
    pub hover_provider: bool,
    #[serde(default)]
    pub definition_provider: bool,
    #[serde(default)]
    pub references_provider: bool,
}

/// Initialize response result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub capabilities: ServerCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

// ---------------------------------------------------------------------------
// LSP-specific types — Text Document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
    pub uri: String,
    pub language_id: String,
    pub version: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidOpenTextDocumentParams {
    pub text_document: TextDocumentItem,
}

/// Full-sync content change — we send the full text each time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeTextDocumentParams {
    pub text_document: VersionedTextDocumentIdentifier,
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentContentChangeEvent {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidSaveTextDocumentParams {
    pub text_document: TextDocumentIdentifier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidCloseTextDocumentParams {
    pub text_document: TextDocumentIdentifier,
}

// ---------------------------------------------------------------------------
// Position, Range, Location — used by navigation/diagnostics requests
// ---------------------------------------------------------------------------

/// Zero-based line and character position (UTF-16 code units per spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentPositionParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

// ---------------------------------------------------------------------------
// Definition / References / Hover
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceContext {
    pub include_declaration: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    pub context: ReferenceContext,
}

/// Response from textDocument/definition — server can return a single Location,
/// an array, or null. We normalize to Vec<Location>.
#[derive(Debug, Clone)]
pub struct DefinitionResponse {
    pub locations: Vec<Location>,
}

impl DefinitionResponse {
    /// Parse definition response — handles single Location, Vec<Location>, or null.
    pub fn from_value(v: serde_json::Value) -> Self {
        if v.is_null() {
            return DefinitionResponse { locations: vec![] };
        }
        if let Ok(single) = serde_json::from_value::<Location>(v.clone()) {
            return DefinitionResponse {
                locations: vec![single],
            };
        }
        if let Ok(multi) = serde_json::from_value::<Vec<Location>>(v) {
            return DefinitionResponse { locations: multi };
        }
        DefinitionResponse { locations: vec![] }
    }
}

/// Response from textDocument/references — always a Vec<Location> or null.
#[derive(Debug, Clone)]
pub struct ReferencesResponse {
    pub locations: Vec<Location>,
}

impl ReferencesResponse {
    pub fn from_value(v: serde_json::Value) -> Self {
        if v.is_null() {
            return ReferencesResponse { locations: vec![] };
        }
        if let Ok(multi) = serde_json::from_value::<Vec<Location>>(v) {
            return ReferencesResponse { locations: multi };
        }
        ReferencesResponse { locations: vec![] }
    }
}

/// Hover contents — LSP allows several shapes: MarkupContent, MarkedString,
/// or an array of MarkedString. We flatten all variants to plain text.
#[derive(Debug, Clone)]
pub struct HoverResponse {
    pub contents: String,
    pub range: Option<Range>,
}

impl HoverResponse {
    /// Parse hover response from arbitrary JSON.
    ///
    /// LSP spec variants:
    /// - `{ contents: "string", range?: ... }`
    /// - `{ contents: { kind: "markdown"|"plaintext", value: "..." }, range?: ... }`
    /// - `{ contents: { language: "rust", value: "..." }, range?: ... }` (deprecated MarkedString)
    /// - `{ contents: [MarkedString, ...], range?: ... }` (deprecated array form)
    /// - `null` → no hover info
    pub fn from_value(v: serde_json::Value) -> Self {
        if v.is_null() {
            return HoverResponse {
                contents: String::new(),
                range: None,
            };
        }

        let range = v
            .get("range")
            .cloned()
            .and_then(|r| serde_json::from_value::<Range>(r).ok());

        let contents = v
            .get("contents")
            .map(flatten_hover_contents)
            .unwrap_or_default();

        HoverResponse { contents, range }
    }
}

/// Recursively flatten hover `contents` JSON into plain text.
fn flatten_hover_contents(v: &serde_json::Value) -> String {
    // String
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    // Array of MarkedString / MarkupContent
    if let Some(arr) = v.as_array() {
        return arr
            .iter()
            .map(flatten_hover_contents)
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    // Object: MarkupContent { kind, value } or MarkedString { language, value }
    if let Some(obj) = v.as_object() {
        if let Some(value) = obj.get("value").and_then(|v| v.as_str()) {
            return value.to_string();
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Text document sync kind (from server capabilities)
// ---------------------------------------------------------------------------

/// How the server wants document changes: none, full, or incremental.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDocumentSyncKind {
    None = 0,
    Full = 1,
    Incremental = 2,
}

impl TextDocumentSyncKind {
    pub fn from_value(v: &serde_json::Value) -> Self {
        // Can be a number directly, or an object with { "change": N }
        if let Some(n) = v.as_i64() {
            return Self::from_i64(n);
        }
        if let Some(obj) = v.as_object() {
            if let Some(change) = obj.get("change").and_then(|c| c.as_i64()) {
                return Self::from_i64(change);
            }
        }
        TextDocumentSyncKind::None
    }

    fn from_i64(n: i64) -> Self {
        match n {
            1 => TextDocumentSyncKind::Full,
            2 => TextDocumentSyncKind::Incremental,
            _ => TextDocumentSyncKind::None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serde_round_trip() {
        let req = Request::new(1, "initialize", Some(serde_json::json!({"processId": 42})));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"initialize\""));

        let parsed: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, RequestId::Integer(1));
        assert_eq!(parsed.method, "initialize");
    }

    #[test]
    fn notification_has_no_id() {
        let notif = Notification::new("initialized", None);
        let json = serde_json::to_string(&notif).unwrap();
        assert!(!json.contains("\"id\""));
        assert!(json.contains("\"method\":\"initialized\""));
    }

    #[test]
    fn response_ok_serde() {
        let resp = Response::ok(
            RequestId::Integer(1),
            serde_json::json!({"capabilities": {}}),
        );
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"result\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn response_error_serde() {
        let resp = Response::error(RequestId::Integer(1), -32600, "Invalid Request");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"error\""));
        assert!(json.contains("-32600"));
        assert!(!json.contains("\"result\""));
    }

    #[test]
    fn message_parses_request() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, Message::Request(_)));
    }

    #[test]
    fn message_parses_notification() {
        let json = r#"{"jsonrpc":"2.0","method":"initialized"}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, Message::Notification(_)));
    }

    #[test]
    fn message_parses_response() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, Message::Response(_)));
    }

    #[test]
    fn initialize_params_camel_case() {
        let params = InitializeParams {
            process_id: Some(1234),
            root_uri: Some("file:///home/user/project".into()),
            capabilities: ClientCapabilities::default(),
            client_info: Some(ClientInfo {
                name: "MAE".into(),
                version: Some("0.1.0".into()),
            }),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("processId"));
        assert!(json.contains("rootUri"));
        assert!(json.contains("clientInfo"));
    }

    #[test]
    fn text_document_sync_kind_from_number() {
        assert_eq!(
            TextDocumentSyncKind::from_value(&serde_json::json!(1)),
            TextDocumentSyncKind::Full,
        );
        assert_eq!(
            TextDocumentSyncKind::from_value(&serde_json::json!(2)),
            TextDocumentSyncKind::Incremental,
        );
    }

    #[test]
    fn text_document_sync_kind_from_object() {
        let v = serde_json::json!({"openClose": true, "change": 1});
        assert_eq!(
            TextDocumentSyncKind::from_value(&v),
            TextDocumentSyncKind::Full,
        );
    }

    #[test]
    fn did_open_params_serde() {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: "file:///test.rs".into(),
                language_id: "rust".into(),
                version: 0,
                text: "fn main() {}".into(),
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("textDocument"));
        assert!(json.contains("languageId"));
    }

    #[test]
    fn position_range_location_serde() {
        let loc = Location {
            uri: "file:///test.rs".into(),
            range: Range {
                start: Position {
                    line: 10,
                    character: 4,
                },
                end: Position {
                    line: 10,
                    character: 12,
                },
            },
        };
        let json = serde_json::to_string(&loc).unwrap();
        assert!(json.contains("\"line\":10"));
        assert!(json.contains("\"character\":4"));
        let parsed: Location = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.uri, "file:///test.rs");
        assert_eq!(parsed.range.start.line, 10);
    }

    #[test]
    fn definition_response_single_location() {
        let v = serde_json::json!({
            "uri": "file:///test.rs",
            "range": {
                "start": {"line": 1, "character": 0},
                "end":   {"line": 1, "character": 5}
            }
        });
        let resp = DefinitionResponse::from_value(v);
        assert_eq!(resp.locations.len(), 1);
        assert_eq!(resp.locations[0].uri, "file:///test.rs");
    }

    #[test]
    fn definition_response_location_array() {
        let v = serde_json::json!([
            {
                "uri": "file:///a.rs",
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}
            },
            {
                "uri": "file:///b.rs",
                "range": {"start": {"line": 5, "character": 0}, "end": {"line": 5, "character": 1}}
            }
        ]);
        let resp = DefinitionResponse::from_value(v);
        assert_eq!(resp.locations.len(), 2);
    }

    #[test]
    fn definition_response_null() {
        let resp = DefinitionResponse::from_value(serde_json::Value::Null);
        assert!(resp.locations.is_empty());
    }

    #[test]
    fn references_response_vec() {
        let v = serde_json::json!([
            {
                "uri": "file:///a.rs",
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}
            }
        ]);
        let resp = ReferencesResponse::from_value(v);
        assert_eq!(resp.locations.len(), 1);
    }

    #[test]
    fn hover_response_string_contents() {
        let v = serde_json::json!({"contents": "Hello world"});
        let resp = HoverResponse::from_value(v);
        assert_eq!(resp.contents, "Hello world");
        assert!(resp.range.is_none());
    }

    #[test]
    fn hover_response_markup_content() {
        let v = serde_json::json!({
            "contents": {"kind": "markdown", "value": "**bold** type: `i32`"},
            "range": {
                "start": {"line": 0, "character": 0},
                "end":   {"line": 0, "character": 5}
            }
        });
        let resp = HoverResponse::from_value(v);
        assert_eq!(resp.contents, "**bold** type: `i32`");
        assert!(resp.range.is_some());
    }

    #[test]
    fn hover_response_marked_string_array() {
        let v = serde_json::json!({
            "contents": [
                {"language": "rust", "value": "fn foo()"},
                "extra docs"
            ]
        });
        let resp = HoverResponse::from_value(v);
        assert!(resp.contents.contains("fn foo()"));
        assert!(resp.contents.contains("extra docs"));
    }

    #[test]
    fn hover_response_null() {
        let resp = HoverResponse::from_value(serde_json::Value::Null);
        assert!(resp.contents.is_empty());
    }

    #[test]
    fn reference_params_camel_case() {
        let params = ReferenceParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///test.rs".into(),
            },
            position: Position {
                line: 10,
                character: 4,
            },
            context: ReferenceContext {
                include_declaration: true,
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("textDocument"));
        assert!(json.contains("includeDeclaration"));
    }

    #[test]
    fn request_id_integer_and_string() {
        let int_id = RequestId::Integer(42);
        let str_id = RequestId::String("abc".into());
        assert_ne!(int_id, str_id);

        let json_int = serde_json::to_string(&int_id).unwrap();
        assert_eq!(json_int, "42");

        let json_str = serde_json::to_string(&str_id).unwrap();
        assert_eq!(json_str, "\"abc\"");
    }
}
