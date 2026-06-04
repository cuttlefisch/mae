//! JSON-RPC method dispatch for daemon requests.
//!
//! Reuses `mae_mcp::{read_message, write_framed}` — same Content-Length
//! framing as the MCP server and state-server.

use mae_kb::query::KbQueryLayer;
use mae_kb::store::SearchHit;
use mae_kb::CozoKbStore;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

/// Daemon state shared across handler invocations.
pub struct DaemonState {
    /// Primary CozoDB store (SQLite backend).
    pub store: Option<Arc<CozoKbStore>>,
    /// Federated query layer across all stores.
    pub query_layer: Option<Arc<dyn KbQueryLayer>>,
    /// Federation registry.
    pub registry: mae_kb::federation::KbRegistry,
    /// Instance stores keyed by UUID.
    pub instance_stores: std::collections::HashMap<String, Arc<CozoKbStore>>,
    /// Daemon startup time.
    pub started_at: Instant,
}

impl DaemonState {
    pub fn new() -> Self {
        Self {
            store: None,
            query_layer: None,
            registry: mae_kb::federation::KbRegistry::default(),
            instance_stores: std::collections::HashMap::new(),
            started_at: Instant::now(),
        }
    }

    /// Rebuild the federated query layer from current stores.
    pub fn rebuild_query_layer(&mut self) {
        if let Some(ref store) = self.store {
            let primary = Arc::new(mae_kb::CozoQueryLayer::new(Arc::clone(store)));
            let mut federated = mae_kb::FederatedQuery::new(primary);
            for (name, inst_store) in &self.instance_stores {
                let layer = Arc::new(mae_kb::CozoQueryLayer::new(Arc::clone(inst_store)));
                federated.add_instance(name.clone(), layer);
            }
            self.query_layer = Some(Arc::new(federated));
        }
    }
}

/// Dispatch a JSON-RPC request and return the result value.
pub async fn dispatch(
    method: &str,
    params: Value,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<Value, DaemonError> {
    match method {
        // --- KB queries ---
        "kb/get" => {
            let id = params["id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?;
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            match ql.get(id) {
                Some(node) => Ok(json!({
                    "id": node.id,
                    "title": node.title,
                    "kind": format!("{:?}", node.kind),
                    "body": node.body,
                    "tags": node.tags,
                })),
                None => Ok(Value::Null),
            }
        }

        "kb/search" => {
            let query = params["query"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'query'"))?;
            let limit = params["limit"].as_u64().unwrap_or(20) as usize;
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            let hits: Vec<Value> = ql
                .search(query, limit)
                .into_iter()
                .map(|h: SearchHit| json!({ "id": h.id, "score": h.score }))
                .collect();
            Ok(json!(hits))
        }

        "kb/links_from" => {
            let id = params["id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?;
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            let links: Vec<Value> = ql
                .links_from(id)
                .into_iter()
                .map(|l| {
                    json!({
                        "src": l.src,
                        "dst": l.dst,
                        "rel_type": l.rel_type,
                    })
                })
                .collect();
            Ok(json!(links))
        }

        "kb/links_to" => {
            let id = params["id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?;
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            let links: Vec<Value> = ql
                .links_to(id)
                .into_iter()
                .map(|l| {
                    json!({
                        "src": l.src,
                        "dst": l.dst,
                        "rel_type": l.rel_type,
                    })
                })
                .collect();
            Ok(json!(links))
        }

        "kb/list_ids" => {
            let prefix = params["prefix"].as_str();
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            Ok(json!(ql.list_ids(prefix)))
        }

        "kb/health" => {
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            match ql.health_report() {
                Some(report) => Ok(json!({
                    "total_nodes": report.total_nodes,
                    "total_links": report.total_links,
                    "orphan_count": report.orphan_ids.len(),
                    "broken_link_count": report.broken_links.len(),
                })),
                None => Ok(json!({"error": "health report unavailable"})),
            }
        }

        "kb/id_title_pairs" => {
            let prefix = params["prefix"].as_str();
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            let pairs: Vec<Value> = ql
                .id_title_pairs(prefix)
                .into_iter()
                .map(|(id, title)| json!([id, title]))
                .collect();
            Ok(json!(pairs))
        }

        "kb/id_title_body_triples" => {
            let prefix = params["prefix"].as_str();
            let body_limit = params["body_limit"].as_u64().unwrap_or(0) as usize;
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            let triples: Vec<Value> = ql
                .id_title_body_triples(prefix, body_limit)
                .into_iter()
                .map(|(id, title, body)| json!([id, title, body]))
                .collect();
            Ok(json!(triples))
        }

        // --- Lifecycle ---
        "daemon/status" => {
            let state = state.lock().await;
            let uptime = state.started_at.elapsed();
            let store_count = 1 + state.instance_stores.len();
            Ok(json!({
                "uptime_secs": uptime.as_secs(),
                "stores": store_count,
                "has_query_layer": state.query_layer.is_some(),
                "registered_instances": state.registry.instances.len(),
            }))
        }

        "daemon/shutdown" => Ok(json!({"shutting_down": true})),

        _ => Err(DaemonError::MethodNotFound(method.to_string())),
    }
}

/// Daemon-specific errors.
#[derive(Debug)]
pub enum DaemonError {
    InvalidParams(&'static str),
    NotReady,
    MethodNotFound(String),
    #[allow(dead_code)]
    Internal(String),
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonError::InvalidParams(msg) => write!(f, "Invalid params: {msg}"),
            DaemonError::NotReady => write!(f, "Daemon not ready (no KB store loaded)"),
            DaemonError::MethodNotFound(m) => write!(f, "Method not found: {m}"),
            DaemonError::Internal(msg) => write!(f, "Internal error: {msg}"),
        }
    }
}

impl DaemonError {
    /// JSON-RPC error code.
    pub fn code(&self) -> i64 {
        match self {
            DaemonError::InvalidParams(_) => -32602,
            DaemonError::NotReady => -32603,
            DaemonError::MethodNotFound(_) => -32601,
            DaemonError::Internal(_) => -32603,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_returns_uptime() {
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch("daemon/status", json!({}), &state).await.unwrap();
        assert!(result["uptime_secs"].as_u64().is_some());
        assert_eq!(result["stores"].as_u64(), Some(1));
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch("nonexistent/method", json!({}), &state).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn kb_get_without_store_returns_not_ready() {
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch("kb/get", json!({"id": "test:node"}), &state).await;
        assert!(matches!(result, Err(DaemonError::NotReady)));
    }
}
