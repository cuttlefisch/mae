//! DAP (Debug Adapter Protocol) wire types.
//!
//! DAP uses the same Content-Length framing as LSP but its own message envelope
//! (seq/type/command) rather than JSON-RPC. Messages come in three flavors:
//! Request, Response, and Event.
//!
//! We define typed argument/body structs for the subset of commands we need,
//! plus conversion functions to map DAP types → mae-core debug types.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Base message envelope
// ---------------------------------------------------------------------------

/// Top-level DAP message — tagged by `type`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DapMessage {
    #[serde(rename = "request")]
    Request(DapRequest),
    #[serde(rename = "response")]
    Response(DapResponse),
    #[serde(rename = "event")]
    Event(DapEvent),
}

/// A DAP request (client → adapter).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapRequest {
    pub seq: i64,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// A DAP response (adapter → client).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapResponse {
    pub seq: i64,
    pub request_seq: i64,
    pub success: bool,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

/// A DAP event (adapter → client).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapEvent {
    pub seq: i64,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Typed argument/body structs — initialize
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequestArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
    #[serde(default)]
    pub lines_start_at1: bool,
    #[serde(default)]
    pub columns_start_at1: bool,
    #[serde(default)]
    pub supports_variable_type: bool,
    #[serde(default)]
    pub supports_variable_paging: bool,
    #[serde(default)]
    pub supports_run_in_terminal_request: bool,
    #[serde(default)]
    pub supports_memory_references: bool,
    #[serde(default)]
    pub supports_progress_reporting: bool,
    #[serde(default)]
    pub supports_invalidated_event: bool,
}

/// Capabilities returned by the adapter in the initialize response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    #[serde(default)]
    pub supports_configuration_done_request: bool,
    #[serde(default)]
    pub supports_function_breakpoints: bool,
    #[serde(default)]
    pub supports_conditional_breakpoints: bool,
    #[serde(default)]
    pub supports_evaluate_for_hovers: bool,
    #[serde(default)]
    pub supports_set_variable: bool,
    #[serde(default)]
    pub supports_step_back: bool,
    #[serde(default)]
    pub supports_restart_frame: bool,
    #[serde(default)]
    pub supports_terminate_request: bool,
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequestArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub no_debug: bool,
    /// Adapter-specific pass-through data.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "__restart")]
    pub restart: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Breakpoints
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBreakpoint {
    pub line: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsArguments {
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoints: Option<Vec<SourceBreakpoint>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsResponseBody {
    pub breakpoints: Vec<DapBreakpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DapBreakpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

// ---------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsResponseBody {
    pub threads: Vec<DapThread>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DapThread {
    pub id: i64,
    pub name: String,
}

// ---------------------------------------------------------------------------
// Stack trace
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTraceArguments {
    pub thread_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_frame: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTraceResponseBody {
    pub stack_frames: Vec<DapStackFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_frames: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DapStackFrame {
    pub id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    pub line: i64,
    pub column: i64,
}

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopesArguments {
    pub frame_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopesResponseBody {
    pub scopes: Vec<DapScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DapScope {
    pub name: String,
    pub variables_reference: i64,
    #[serde(default)]
    pub expensive: bool,
}

// ---------------------------------------------------------------------------
// Variables
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablesArguments {
    pub variables_reference: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablesResponseBody {
    pub variables: Vec<DapVariable>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DapVariable {
    pub name: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub type_field: Option<String>,
    #[serde(default)]
    pub variables_reference: i64,
}

// ---------------------------------------------------------------------------
// Evaluate
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateArguments {
    pub expression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateResponseBody {
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub type_field: Option<String>,
    #[serde(default)]
    pub variables_reference: i64,
}

// ---------------------------------------------------------------------------
// Execution control arguments
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueArguments {
    pub thread_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextArguments {
    pub thread_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepInArguments {
    pub thread_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepOutArguments {
    pub thread_id: i64,
}

// ---------------------------------------------------------------------------
// Event bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoppedEventBody {
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEventBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminatedEventBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitedEventBody {
    pub exit_code: i64,
}

// ---------------------------------------------------------------------------
// Conversion: DAP types → mae-core debug types
// ---------------------------------------------------------------------------

impl DapThread {
    /// Convert to mae-core DebugThread. DAP threads are assumed running
    /// unless we've received a stopped event for them.
    pub fn to_core(&self) -> mae_core::DebugThread {
        mae_core::DebugThread {
            id: self.id,
            name: self.name.clone(),
            stopped: false,
        }
    }
}

impl DapStackFrame {
    pub fn to_core(&self) -> mae_core::StackFrame {
        mae_core::StackFrame {
            id: self.id,
            name: self.name.clone(),
            source: self.source.as_ref().and_then(|s| {
                s.path.clone().or_else(|| s.name.clone())
            }),
            line: self.line,
            column: self.column,
        }
    }
}

impl DapScope {
    pub fn to_core(&self) -> mae_core::Scope {
        mae_core::Scope {
            name: self.name.clone(),
            variables_reference: self.variables_reference,
            expensive: self.expensive,
        }
    }
}

impl DapVariable {
    pub fn to_core(&self) -> mae_core::Variable {
        mae_core::Variable {
            name: self.name.clone(),
            value: self.value.clone(),
            var_type: self.type_field.clone(),
            variables_reference: self.variables_reference,
        }
    }
}

impl DapBreakpoint {
    pub fn to_core(&self) -> mae_core::Breakpoint {
        mae_core::Breakpoint {
            id: self.id.unwrap_or(0),
            verified: self.verified,
            source: self.source.as_ref()
                .and_then(|s| s.path.clone().or_else(|| s.name.clone()))
                .unwrap_or_default(),
            line: self.line.unwrap_or(0),
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
        let msg = DapMessage::Request(DapRequest {
            seq: 1,
            command: "initialize".into(),
            arguments: Some(serde_json::json!({"clientID": "mae"})),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: DapMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            DapMessage::Request(req) => {
                assert_eq!(req.seq, 1);
                assert_eq!(req.command, "initialize");
                assert!(req.arguments.is_some());
            }
            _ => panic!("expected Request"),
        }
    }

    #[test]
    fn response_serde_round_trip() {
        let msg = DapMessage::Response(DapResponse {
            seq: 2,
            request_seq: 1,
            success: true,
            command: "initialize".into(),
            message: None,
            body: Some(serde_json::json!({"supportsConfigurationDoneRequest": true})),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: DapMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            DapMessage::Response(resp) => {
                assert_eq!(resp.seq, 2);
                assert_eq!(resp.request_seq, 1);
                assert!(resp.success);
                assert!(resp.body.is_some());
            }
            _ => panic!("expected Response"),
        }
    }

    #[test]
    fn event_serde_round_trip() {
        let msg = DapMessage::Event(DapEvent {
            seq: 3,
            event: "stopped".into(),
            body: Some(serde_json::json!({"reason": "breakpoint", "threadId": 1})),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: DapMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            DapMessage::Event(evt) => {
                assert_eq!(evt.seq, 3);
                assert_eq!(evt.event, "stopped");
                assert!(evt.body.is_some());
            }
            _ => panic!("expected Event"),
        }
    }

    #[test]
    fn initialize_args_camel_case() {
        let args = InitializeRequestArguments {
            client_id: Some("mae".into()),
            client_name: Some("MAE Editor".into()),
            adapter_id: None,
            lines_start_at1: true,
            columns_start_at1: true,
            supports_variable_type: true,
            supports_variable_paging: false,
            supports_run_in_terminal_request: false,
            supports_memory_references: false,
            supports_progress_reporting: false,
            supports_invalidated_event: false,
        };
        let json = serde_json::to_string(&args).unwrap();
        assert!(json.contains("clientId"));
        assert!(json.contains("clientName"));
        assert!(json.contains("linesStartAt1"));
        assert!(json.contains("columnsStartAt1"));
        assert!(json.contains("supportsVariableType"));
    }

    #[test]
    fn stopped_event_body_parse() {
        let json = r#"{"reason":"breakpoint","threadId":1,"text":"Hit breakpoint at main.rs:42"}"#;
        let body: StoppedEventBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.reason, "breakpoint");
        assert_eq!(body.thread_id, Some(1));
        assert_eq!(body.text.as_deref(), Some("Hit breakpoint at main.rs:42"));
    }

    #[test]
    fn dap_thread_to_core_thread() {
        let dap_thread = DapThread {
            id: 42,
            name: "main".into(),
        };
        let core_thread = dap_thread.to_core();
        assert_eq!(core_thread.id, 42);
        assert_eq!(core_thread.name, "main");
        assert!(!core_thread.stopped);
    }
}
