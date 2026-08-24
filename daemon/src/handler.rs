//! JSON-RPC method dispatch for daemon requests.
//!
//! Reuses `mae_mcp::{read_message, write_framed}` — same Content-Length
//! framing as the MCP server and collab server.

use mae_kb::query::KbQueryLayer;
use mae_kb::store::SearchHit;
use mae_kb::{CozoKbStore, KbStore};
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
    /// The P2P mesh endpoint (ADR-025), present only when `collab.p2p.enabled`.
    /// Stored here so the local control socket can mint join tickets
    /// (`p2p/mint_ticket`) without reaching into the collab session machinery.
    /// `Endpoint` is cheaply cloneable (Arc-backed); the accept loop owns its
    /// own clone.
    pub p2p_endpoint: Option<iroh::Endpoint>,
    /// Join targets accepted from `p2p/join_ticket` (parsed "magnet links"): the
    /// owner `EndpointAddr` + KB-id the Phase-2 mesh dialer will dial. Recorded
    /// here now; the dial + TOFU trust happen when the dialer lands (#89).
    pub pending_p2p_joins: Vec<crate::ticket::JoinTicket>,
    /// The collab server's collaborative-document store (kbc:*/node docs), shared
    /// with the TCP/mesh listeners. Present once `spawn_collab_server` has wired it
    /// in. Lets the local control socket *establish* a P2P share (`p2p/share_kb`)
    /// — create/widen the collection doc to the mesh — without a collab session.
    pub doc_store: Option<Arc<mae_daemon::doc_store::DocStore>>,
    /// The collab event broadcaster, so a control-socket share is observed by
    /// connected sync sessions (parity with `kb/share` over TCP).
    pub broadcaster: Option<mae_mcp::broadcast::SharedBroadcaster>,
    /// This daemon's key-mode identity — the OWNER principal stamped on any
    /// collection established via `p2p/share_kb` (mirrors the TCP `kb/share`
    /// owner-binding to the authenticated principal).
    pub owner: Option<Arc<mae_mcp::identity::Identity>>,
    /// ADR-060 Phase C: per-tenant quota/eviction registry. An `Arc`, not
    /// inlined state — `TenantRegistry` is internally concurrent (`dashmap`),
    /// so cloning the `Arc` out under this brief lock (the same pattern
    /// `store`/`query_layer`/`doc_store` already use) never reintroduces the
    /// per-request contention Phase B proved this lock must stay free of.
    /// Defaults to `TenantRegistry::empty()` (zero `[[tenant]]` tables = zero
    /// behavior change) until `main()` replaces it with the real one built
    /// from loaded config.
    pub tenants: Arc<crate::tenant::TenantRegistry>,
    /// Live-connection counters for the two listeners a client can arrive on,
    /// so `daemon/status` can answer "how many clients are connected right now".
    ///
    /// @ai-caution: [observability] These are `ConnLimiter` clones (Arc-backed
    /// atomics), NOT the broadcaster's session map. The broadcaster is only
    /// installed into `DaemonState` under `collab.auth.mode = "key"`
    /// (`main.rs`'s auth match), so a count sourced from it silently reports
    /// zero under `psk`/`none` — exactly the modes a quick hub deployment is
    /// most likely to be running. The limiters are wired unconditionally at
    /// listener creation and do not have that hole.
    pub kb_conn: Option<crate::conn_limit::ConnLimiter>,
    pub collab_conn: Option<crate::conn_limit::ConnLimiter>,
}

/// How many clients are attached to this daemon right now, per listener.
///
/// Reported by `daemon/status`. Before this existed there was no way to observe
/// that a hub had any clients at all: `daemon/status` reported uptime, stores,
/// instances and live tenants, and the only connection counts anywhere were
/// per-document (`docs/metadata`) or broadcast to subscribers of one document.
/// "Three editors are connected to the hub" was not a checkable claim.
///
/// Each listener reports `active` (accepted, not yet closed) and `max` (0 =
/// unlimited). A listener that isn't running is absent rather than zero — zero
/// connections and no listener are different facts, and conflating them is how
/// a disabled collab server reads as a healthy idle one.
///
/// `collab.sessions` is a *different* number from `collab.active`: `active`
/// counts accepted TCP connections, `sessions` counts sync sessions that got
/// past authentication and subscribed. A gap between them across successive
/// polls means clients are connecting and failing to authenticate — the single
/// most useful thing to see while bringing a hub up.
fn connection_report(state: &DaemonState) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (name, limiter) in [
        ("kb_socket", &state.kb_conn),
        ("collab", &state.collab_conn),
    ] {
        if let Some(l) = limiter {
            out.insert(
                name.to_string(),
                json!({"active": l.current(), "max": l.max()}),
            );
        }
    }
    // Authenticated collab sync sessions. `client_count` reaps dead channels
    // only during a broadcast, so between broadcasts it can over-report a
    // client whose socket already closed without unsubscribing — it is a
    // liveness signal, not an exact figure, and `collab.active` is the
    // authoritative connection count.
    if let Some(bc) = &state.broadcaster {
        let n = bc.lock().unwrap_or_else(|e| e.into_inner()).client_count();
        if let Some(collab) = out.get_mut("collab").and_then(|v| v.as_object_mut()) {
            collab.insert("sessions".to_string(), json!(n));
        }
    }
    serde_json::Value::Object(out)
}

impl DaemonState {
    pub fn new() -> Self {
        Self {
            store: None,
            query_layer: None,
            registry: mae_kb::federation::KbRegistry::default(),
            instance_stores: std::collections::HashMap::new(),
            started_at: Instant::now(),
            p2p_endpoint: None,
            pending_p2p_joins: Vec::new(),
            doc_store: None,
            broadcaster: None,
            owner: None,
            tenants: Arc::new(crate::tenant::TenantRegistry::empty()),
            kb_conn: None,
            collab_conn: None,
        }
    }

    /// Rebuild the federated query layer from current stores.
    pub fn rebuild_query_layer(&mut self) {
        if let Some(ref store) = self.store {
            let primary = Arc::new(mae_kb::CozoQueryLayer::new(Arc::clone(store)));
            let mut federated = mae_kb::FederatedQuery::new(primary);
            for (name, inst_store) in &self.instance_stores {
                let layer = Arc::new(mae_kb::CozoQueryLayer::new(Arc::clone(inst_store)));
                // Priority (ADR-062 Phase B) comes from the registry entry when one
                // exists; falls back to the default (0) equal weight otherwise.
                let priority = self
                    .registry
                    .find(name)
                    .map(|inst| inst.priority)
                    .unwrap_or(0);
                federated.add_instance(name.clone(), priority, layer);
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
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?
                .to_string();
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Read,
            )
            .await?;
            spawn_query(move || match ql.get(&id) {
                Some(node) => json!({
                    "id": node.id,
                    "title": node.title,
                    "kind": node.kind.as_str(),
                    "body": node.body,
                    "tags": node.tags,
                }),
                None => Value::Null,
            })
            .await
        }

        "kb/search" => {
            let query = params["query"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'query'"))?
                .to_string();
            let limit = std::cmp::min(params["limit"].as_u64().unwrap_or(20), 1000) as usize;
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Scan,
            )
            .await?;
            spawn_query_result(move || {
                let hits: Vec<Value> = ql
                    .search(&query, limit)
                    .map_err(|e| DaemonError::Internal(e.to_string()))?
                    .into_iter()
                    .map(|h: SearchHit| json!({ "id": h.id, "score": h.score }))
                    .collect();
                Ok(json!(hits))
            })
            .await
        }

        "kb/links_from" => {
            let id = params["id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?
                .to_string();
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Read,
            )
            .await?;
            spawn_query_result(move || {
                let links: Vec<Value> = ql
                    .links_from(&id)
                    .map_err(|e| DaemonError::Internal(e.to_string()))?
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
            })
            .await
        }

        "kb/links_to" => {
            let id = params["id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?
                .to_string();
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Read,
            )
            .await?;
            spawn_query_result(move || {
                let links: Vec<Value> = ql
                    .links_to(&id)
                    .map_err(|e| DaemonError::Internal(e.to_string()))?
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
            })
            .await
        }

        "kb/list_ids" => {
            let prefix = params["prefix"].as_str().map(|s| s.to_string());
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Read,
            )
            .await?;
            spawn_query_result(move || {
                ql.list_ids(prefix.as_deref())
                    .map(|ids| json!(ids))
                    .map_err(|e| DaemonError::Internal(e.to_string()))
            })
            .await
        }

        "kb/health" => {
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Read,
            )
            .await?;
            spawn_query_result(move || {
                let report = ql
                    .health_report()
                    .map_err(|e| DaemonError::Internal(e.to_string()))?;
                Ok(match report {
                    Some(report) => json!({
                        "total_nodes": report.total_nodes,
                        "total_links": report.total_links,
                        "orphan_count": report.orphan_ids.len(),
                        "broken_link_count": report.broken_links.len(),
                    }),
                    None => json!({"error": "health report unavailable"}),
                })
            })
            .await
        }

        "kb/neighborhood" => {
            let id = params["id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?
                .to_string();
            let depth = params["depth"].as_u64().unwrap_or(1) as u32;
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Scan,
            )
            .await?;
            spawn_query_result(move || {
                let sg = ql
                    .neighborhood(&id, depth)
                    .map_err(|e| DaemonError::Internal(e.to_string()))?;
                Ok(match sg {
                    Some(sg) => json!({
                        "nodes": sg.nodes.iter().map(|(id, t)| json!([id, t])).collect::<Vec<_>>(),
                        "edges": sg.edges.iter().map(|(s, d, r)| json!([s, d, r])).collect::<Vec<_>>(),
                    }),
                    None => json!({"nodes": [], "edges": []}),
                })
            })
            .await
        }

        "kb/related" => {
            let id = params["id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?
                .to_string();
            let limit = std::cmp::min(params["limit"].as_u64().unwrap_or(10), 1000) as usize;
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Scan,
            )
            .await?;
            spawn_query_result(move || {
                let related: Vec<Value> = ql
                    .related(&id, limit)
                    .map_err(|e| DaemonError::Internal(e.to_string()))?
                    .into_iter()
                    .map(|(id, score)| json!([id, score]))
                    .collect();
                Ok(json!(related))
            })
            .await
        }

        // Phase D3b (ADR-029): return a node's authoritative CRDT doc state from the
        // doc_store so a thin-client editor can lazily hydrate the node — with its
        // real lineage — into its edit mirror (the daemon is the source of truth, so
        // the editor neither reads nor writes its own cozo for the hosted primary).
        // Returns `null` for a node the daemon doesn't host (so the editor doesn't
        // materialize an empty node).
        "kb/node_crdt" => {
            let id = params["id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?;
            // ADR-105: node docs are addressed per-KB, so this method needs the KB.
            // Required, not defaulted: guessing the KB from the node id alone is the
            // ambiguity the scoped address exists to remove.
            let kb_id = params["kb_id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'kb_id'"))?;
            let doc_store = { state.lock().await.doc_store.clone() };
            let ds = doc_store.ok_or(DaemonError::NotReady)?;
            let doc_name = mae_sync::kb_node_doc_name(kb_id, id);
            if !ds.has_durable_doc(&doc_name).await {
                return Ok(Value::Null);
            }
            match ds.encode_state_and_sv(&doc_name).await {
                Ok((node_state, _sv)) => Ok(json!({
                    "state": mae_sync::encoding::update_to_base64(&node_state),
                })),
                Err(e) => Err(DaemonError::Internal(format!("encode '{doc_name}': {e}"))),
            }
        }

        "kb/todo_nodes" => {
            // Phase D thin-client: the agenda buffer was mirror-only. Serve all
            // TODO-bearing nodes as full (serde) nodes — minus the heavy crdt_doc
            // lineage, which the agenda doesn't need.
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Read,
            )
            .await?;
            spawn_query_result(move || {
                let nodes: Vec<Value> = ql
                    .todo_nodes()
                    .map_err(|e| DaemonError::Internal(e.to_string()))?
                    .into_iter()
                    .map(|mut n| {
                        n.crdt_doc = None;
                        serde_json::to_value(&n).unwrap_or(Value::Null)
                    })
                    .collect();
                Ok(json!(nodes))
            })
            .await
        }

        "kb/id_title_pairs" => {
            let prefix = params["prefix"].as_str().map(|s| s.to_string());
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Read,
            )
            .await?;
            spawn_query_result(move || {
                let pairs: Vec<Value> = ql
                    .id_title_pairs(prefix.as_deref())
                    .map_err(|e| DaemonError::Internal(e.to_string()))?
                    .into_iter()
                    .map(|(id, title)| json!([id, title]))
                    .collect();
                Ok(json!(pairs))
            })
            .await
        }

        "kb/id_title_body_triples" => {
            let prefix = params["prefix"].as_str().map(|s| s.to_string());
            let body_limit =
                std::cmp::min(params["body_limit"].as_u64().unwrap_or(0), 10_000) as usize;
            let (ql, _conn_guard) = snapshot_query_layer(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Read,
            )
            .await?;
            spawn_query_result(move || {
                let triples: Vec<Value> = ql
                    .id_title_body_triples(prefix.as_deref(), body_limit)
                    .map_err(|e| DaemonError::Internal(e.to_string()))?
                    .into_iter()
                    .map(|(id, title, body)| json!([id, title, body]))
                    .collect();
                Ok(json!(triples))
            })
            .await
        }

        // --- Hygiene ---
        "kb/hygiene_scan" => {
            let (store, _conn_guard) = snapshot_store(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Mutation,
            )
            .await?;
            spawn_query(move || {
                let result = crate::hygiene::run_hygiene_scan(&store);
                json!({
                    "suggestions_created": result.suggestions_created,
                    "nodes_scanned": result.nodes_scanned,
                    "errors": result.errors,
                })
            })
            .await
        }

        "kb/hygiene_report" => {
            let category = params["category"].as_str().map(|s| s.to_string());
            let status = params["status"].as_str().map(|s| s.to_string());
            let (store, _conn_guard) = snapshot_store(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Read,
            )
            .await?;
            spawn_query_result(move || {
                let suggestions = store
                    .list_suggestions(category.as_deref(), status.as_deref())
                    .map_err(|e| DaemonError::Internal(e.to_string()))?;
                let items: Vec<Value> = suggestions
                    .iter()
                    .map(|s| {
                        json!({
                            "node_id": s.node_id,
                            "suggestion_id": s.suggestion_id,
                            "category": s.category,
                            "message": s.message,
                            "suggested_action": s.suggested_action_json,
                            "confidence": s.confidence,
                            "status": s.status,
                            "created_at": s.created_at,
                        })
                    })
                    .collect();
                Ok(json!(items))
            })
            .await
        }

        "kb/hygiene_accept" => {
            let node_id = params["node_id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'node_id'"))?
                .to_string();
            let suggestion_id = params["suggestion_id"]
                .as_i64()
                .ok_or(DaemonError::InvalidParams("missing 'suggestion_id'"))?;
            let (store, _conn_guard) = snapshot_store(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Mutation,
            )
            .await?;
            spawn_query_result(move || {
                store
                    .update_suggestion_status(&node_id, suggestion_id, "accepted")
                    .map_err(|e| DaemonError::Internal(e.to_string()))?;
                Ok(json!({"ok": true}))
            })
            .await
        }

        "kb/hygiene_dismiss" => {
            let node_id = params["node_id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'node_id'"))?
                .to_string();
            let suggestion_id = params["suggestion_id"]
                .as_i64()
                .ok_or(DaemonError::InvalidParams("missing 'suggestion_id'"))?;
            let (store, _conn_guard) = snapshot_store(
                state,
                instance_addr(&params),
                crate::tenant::RequestCost::Mutation,
            )
            .await?;
            spawn_query_result(move || {
                store
                    .update_suggestion_status(&node_id, suggestion_id, "dismissed")
                    .map_err(|e| DaemonError::Internal(e.to_string()))?;
                Ok(json!({"ok": true}))
            })
            .await
        }

        // --- Lifecycle ---
        "daemon/status" => {
            // Snapshot the fields, then drop the lock before the async doc_store scan
            // (don't hold the state mutex across an await).
            let (uptime, store_count, has_ql, reg_count, doc_store, live_tenants, connections) = {
                let state = state.lock().await;
                (
                    state.started_at.elapsed(),
                    1 + state.instance_stores.len(),
                    state.query_layer.is_some(),
                    state.registry.instances.len(),
                    state.doc_store.clone(),
                    state.tenants.live_tenant_count(),
                    connection_report(&state),
                )
            };
            // Phase D introspection: which KB collections does the daemon host, and
            // does it host the primary (kbc:default)? Lets a connecting editor skip
            // warming its own store and host/route the primary through the daemon.
            let kb_collections = match doc_store {
                Some(ds) => ds.list_collection_ids().await,
                None => Vec::new(),
            };
            // "default" = KB_DEFAULT_NAME (the primary's canonical collab id).
            let primary_exists = kb_collections
                .iter()
                .any(|c| c == "default" || c == "primary");
            Ok(json!({
                // Daemon crate version — the version-skew signal an editor compares
                // against its own before attaching (ADR-035 supervision guardrail;
                // a co-located on-demand daemon must match the editor that spawned
                // it). A finer build-id can layer on later if semver proves coarse.
                "version": env!("CARGO_PKG_VERSION"),
                "uptime_secs": uptime.as_secs(),
                "stores": store_count,
                "has_query_layer": has_ql,
                "registered_instances": reg_count,
                "kb_collections": kb_collections,
                "primary_exists": primary_exists,
                "live_tenants": live_tenants,
                "connections": connections,
            }))
        }

        // ADR-060 Phase C: the operator-triggered eviction escape hatch (the
        // named rust-analyzer/Emacs-precedent lesson — don't wait for an idle
        // window a still-connected tenant may never hit). Idempotent:
        // evicting an unknown/already-idle-evicted tenant is a clean no-op.
        "daemon/evict_tenant" => {
            let name = params["name"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'name'"))?;
            let tenants = { state.lock().await.tenants.clone() };
            tenants.evict(name);
            Ok(json!({"evicted": name}))
        }

        // daemon/shutdown is intercepted by handle_client() before dispatch;
        // this arm exists for completeness if dispatch is called directly.
        "daemon/shutdown" => Ok(json!({"shutting_down": true})),

        // --- P2P mesh (ADR-025) ---
        // Mint a shareable "magnet link" join ticket for a KB the local owner is
        // sharing. This is a LOCAL control op (the daemon owner sharing their own
        // KB over the Unix socket); remote trust is enforced at the mesh accept
        // gate + pending-approve, not here.
        "p2p/mint_ticket" => {
            let kb_id =
                params
                    .get("kb_id")
                    .and_then(|v| v.as_str())
                    .ok_or(DaemonError::InvalidParams(
                        "p2p/mint_ticket requires a string 'kb_id'",
                    ))?;
            let state = state.lock().await;
            let endpoint = state.p2p_endpoint.as_ref().ok_or(DaemonError::NotReady)?;
            let ticket = crate::p2p::mint_ticket(endpoint, kb_id);
            Ok(json!({ "ticket": ticket.to_string(), "kb_id": kb_id }))
        }

        // Accept a join "magnet link": parse + validate it and record the dial
        // target. The actual connect + TOFU trust happen via the Phase-2 dialer
        // (#89) — recorded here so the workflow surface exists now and the dialer
        // consumes `pending_p2p_joins` when it lands.
        "p2p/join_ticket" => {
            let ticket_str =
                params
                    .get("ticket")
                    .and_then(|v| v.as_str())
                    .ok_or(DaemonError::InvalidParams(
                        "p2p/join_ticket requires a string 'ticket'",
                    ))?;
            let ticket: crate::ticket::JoinTicket = ticket_str.trim().parse().map_err(|_| {
                DaemonError::InvalidParams("malformed join ticket (expected mae://join/…)")
            })?;
            let kb_id = ticket.kb_id.clone();
            let node_id = ticket.node_id();
            // The owner's authorized_keys principal (what the dialer will verify).
            let peer = mae_mcp::identity::PublicKey::from_bytes(node_id.as_bytes(), None)
                .map(|k| k.fingerprint())
                .unwrap_or_else(|| "unknown".to_string());
            {
                let mut st = state.lock().await;
                // ADR-086: `run_dialer` (the ONLY consumer of `pending_p2p_joins`) is
                // spawned exclusively alongside a bound P2P endpoint (main.rs) -- with
                // no mesh running there is no dialer to ever service this queue, so
                // the "recorded, will connect" reply below would describe a join that
                // can never actually happen. Refuse before queuing rather than return
                // a success the daemon cannot make good on.
                if st.p2p_endpoint.is_none() {
                    return Err(DaemonError::NotReady);
                }
                // Idempotent: don't queue the same (peer, KB) twice.
                if !st
                    .pending_p2p_joins
                    .iter()
                    .any(|t| t.node_id() == node_id && t.kb_id == kb_id)
                {
                    st.pending_p2p_joins.push(ticket);
                }
            }
            Ok(json!({
                "kb_id": kb_id,
                "peer": peer,
                "status": "recorded",
                "message": format!(
                    "Join recorded for KB '{kb_id}' from peer {peer}. Your daemon's mesh dialer \
                     will connect and pull it once the owner approves your join."
                ),
            }))
        }

        // Establish (or widen) a P2P mesh share for a KB straight from the control
        // socket — the self-sufficient `kb-share-p2p` path (ADR-025 §"Driving
        // surfaces"). Unlike `kb/share` (which needs a collab session carrying the
        // owner's collection), this creates the `kbc:{kb_id}` collection owned by
        // THIS daemon and exposes it on the mesh, so the CLI and the editor command
        // can both share without an open collab session. Mint a ticket afterwards.
        "p2p/share_kb" => {
            let kb_id = params
                .get("kb_id")
                .and_then(|v| v.as_str())
                .ok_or(DaemonError::InvalidParams(
                    "p2p/share_kb requires a string 'kb_id'",
                ))?
                .to_string();
            // This IS the P2P surface, so default exposure = the mesh; callers may
            // pass hub|p2p|both to widen differently.
            let transport = params
                .get("transport")
                .and_then(|v| v.as_str())
                .and_then(mae_sync::kb::TransportPolicy::parse)
                .unwrap_or(mae_sync::kb::TransportPolicy::P2p);
            // Optional join policy (restrictive|invite|permissive). None = leave the
            // collection's default/existing policy untouched.
            let policy = params
                .get("policy")
                .and_then(|v| v.as_str())
                .and_then(mae_sync::kb::JoinPolicy::parse);
            let (doc_store, broadcaster, owner, kb_store) = {
                let st = state.lock().await;
                // @ai-caution: [daemon-state] The actionable diagnostic hangs off
                // `owner`, NOT off `doc_store`. Before #647, doc_store was only
                // populated under key-mode auth, so a psk/none daemon failed on
                // the doc_store check and that check carried the good message.
                // doc_store and broadcaster are now populated for every auth
                // mode, so `owner` is the one that is still None in psk/none —
                // move the message and this stays a useful error instead of
                // "daemon owner identity unavailable".
                let doc_store = st
                    .doc_store
                    .clone()
                    .ok_or_else(|| DaemonError::Internal("collab doc store unavailable".into()))?;
                let broadcaster = st.broadcaster.clone().ok_or_else(|| {
                    DaemonError::Internal("collab broadcaster unavailable".into())
                })?;
                let owner = st.owner.clone().ok_or_else(|| {
                    DaemonError::Internal(
                        "P2P sharing is unavailable — enable collab key mode + the mesh \
                         (`mae setup-collab --p2p`) and restart the daemon"
                            .to_string(),
                    )
                })?;
                // The CozoDB store backing this KB (primary or a named instance), so
                // a fresh share can SEED node content — not just the collection.
                let kb_store = resolve_kb_store(&st, &kb_id);
                (doc_store, broadcaster, owner, kb_store)
            };
            // Build the collection + node states from the daemon's KB store (outside
            // the state lock AND off the async executor — `load_all` is a blocking
            // CozoDB read). Reuses the same `KnowledgeBase::to_collection` the
            // editor's `kb/share` uses, so the seeded node docs are byte-identical.
            // Absent store / empty KB ⇒ an empty collection (still a valid mesh
            // share at collection level).
            let seed = {
                let blocking_kb_id = kb_id.clone();
                let blocking_owner = Arc::clone(&owner);
                tokio::task::spawn_blocking(
                    move || -> Result<Option<SeededCollection>, DaemonError> {
                        match kb_store {
                            Some(store) => {
                                let nodes = store.load_all().map_err(|e| {
                                    DaemonError::Internal(format!("load KB nodes: {e}"))
                                })?;
                                let mut kb = mae_kb::KnowledgeBase::new();
                                for node in nodes {
                                    kb.insert(node);
                                }
                                Ok(Some(
                                    kb.to_collection(
                                        &blocking_kb_id,
                                        &blocking_owner.fingerprint(),
                                        &[],
                                    )
                                    .map_err(|e| {
                                        DaemonError::Internal(format!(
                                            "build collection from KB store: {e}"
                                        ))
                                    })?,
                                ))
                            }
                            None => Ok(None),
                        }
                    },
                )
                .await
                .map_err(|e| DaemonError::Internal(format!("blocking task panicked: {e}")))??
            };
            let (created, node_count) = establish_p2p_share(
                &doc_store,
                &broadcaster,
                &owner,
                &kb_id,
                transport,
                policy,
                seed,
            )
            .await?;
            Ok(json!({
                "kb_id": kb_id,
                "owner": owner.fingerprint(),
                "transport": transport.as_str(),
                "policy": policy.map(|p| p.as_str()),
                "created": created,
                "nodes": node_count,
                "status": "shared",
                "message": format!(
                    "KB '{kb_id}' is shared over the P2P mesh (transport={}{}, {} node{}). \
                     Mint a join ticket to invite a peer.",
                    transport.as_str(),
                    policy
                        .map(|p| format!(", policy={}", p.as_str()))
                        .unwrap_or_default(),
                    node_count,
                    if node_count == 1 { "" } else { "s" },
                ),
            }))
        }

        _ => Err(DaemonError::MethodNotFound(method.to_string())),
    }
}

/// Snapshot the federated query layer `Arc` under the state lock, then drop
/// the lock (ADR-054). This is the read-arm half of the "snapshot-then-drop +
/// spawn_blocking" idiom generalized from the pre-existing `kb/node_crdt` /
/// `daemon/status` / `p2p/share_kb` arms: `DaemonState`'s lock must never be
/// held across the actual (synchronous, potentially slow) CozoDB call.
///
/// ADR-060 Phase A: `addr` is an optional per-request instance address (a KB
/// name or UUID, resolved via [`resolve_kb_store`] — the same address space
/// `instance_stores` already uses, not a new identifier scheme). `None`
/// preserves today's exact single-tenant behavior byte-for-byte: the
/// federated query layer spanning every registered instance. `Some(addr)`
/// scopes the query to exactly that one instance's store — never merged with
/// any other instance's data, which is the property that matters for tenant
/// isolation (a targeted query must never silently see another tenant's
/// results). This is purely additive plumbing: it does not yet change
/// locking or resource accounting (Phase B/C).
async fn snapshot_query_layer(
    state: &Arc<Mutex<DaemonState>>,
    addr: Option<&str>,
    cost: crate::tenant::RequestCost,
) -> Result<(Arc<dyn KbQueryLayer>, Option<crate::conn_limit::ConnGuard>), DaemonError> {
    let state = state.lock().await;
    let guard = charge_tenant_or_reject(&state.tenants, addr, cost)?;
    match addr {
        None => state
            .query_layer
            .clone()
            .ok_or(DaemonError::NotReady)
            .map(|ql| (ql, guard)),
        Some(addr) => {
            let store = resolve_kb_store(&state, addr)
                .ok_or_else(|| DaemonError::UnknownInstance(addr.to_string()))?;
            Ok((Arc::new(mae_kb::CozoQueryLayer::new(store)), guard))
        }
    }
}

/// ADR-060 Phase C enforcement chokepoint: checked inside
/// `snapshot_query_layer`/`snapshot_store`, immediately after acquiring
/// `DaemonState`'s lock but before any of the (comparatively expensive)
/// store-resolution work below it — a rejected request costs only a
/// `dashmap` lookup and a couple of atomic/mutex ops, never a wasted
/// `resolve_kb_store` or the CozoDB call the caller was about to spawn.
/// `addr` unresolved to any `[[tenant]]` entry (including "no tenants
/// configured at all") is `Unconfigured`, which is always admitted — Phase
/// A's own zero-config-zero-behavior-change contract.
fn charge_tenant_or_reject(
    tenants: &crate::tenant::TenantRegistry,
    addr: Option<&str>,
    cost: crate::tenant::RequestCost,
) -> Result<Option<crate::conn_limit::ConnGuard>, DaemonError> {
    use crate::tenant::TenantOutcome;
    let (outcome, guard) = tenants.check_and_charge_by_instance(addr, cost);
    match outcome {
        TenantOutcome::Unconfigured | TenantOutcome::Admitted => Ok(guard),
        TenantOutcome::QuotaExceeded => Err(DaemonError::QuotaExceeded(
            addr.unwrap_or("<unaddressed>").to_string(),
        )),
        TenantOutcome::ConnectionCapExceeded => Err(DaemonError::TenantAtCapacity(
            addr.unwrap_or("<unaddressed>").to_string(),
        )),
    }
}

/// Snapshot a CozoDB store `Arc` under the state lock, then drop the lock —
/// the hygiene arms' equivalent of `snapshot_query_layer` (they need direct
/// store access, not the federated query layer). `addr` behaves identically
/// to `snapshot_query_layer`'s: `None` is today's primary store, `Some(addr)`
/// targets exactly the named/UUID-addressed instance (ADR-060 Phase A).
async fn snapshot_store(
    state: &Arc<Mutex<DaemonState>>,
    addr: Option<&str>,
    cost: crate::tenant::RequestCost,
) -> Result<(Arc<CozoKbStore>, Option<crate::conn_limit::ConnGuard>), DaemonError> {
    let state = state.lock().await;
    let guard = charge_tenant_or_reject(&state.tenants, addr, cost)?;
    match addr {
        None => state
            .store
            .clone()
            .ok_or(DaemonError::NotReady)
            .map(|s| (s, guard)),
        Some(addr) => resolve_kb_store(&state, addr)
            .ok_or_else(|| DaemonError::UnknownInstance(addr.to_string()))
            .map(|s| (s, guard)),
    }
}

/// Extract the optional ADR-060 Phase A instance address from an RPC's
/// `params`. A KB name or UUID (see [`resolve_kb_store`]); absent means
/// "today's primary/federated behavior," never a validation error by itself.
fn instance_addr(params: &Value) -> Option<&str> {
    params.get("instance").and_then(|v| v.as_str())
}

/// Run an infallible synchronous query on a blocking-pool thread, off the
/// async executor (ADR-054) — a synchronous CozoDB call left inline on an
/// async task starves every other connection's I/O sharing that worker.
async fn spawn_query<F>(f: F) -> Result<Value, DaemonError>
where
    F: FnOnce() -> Value + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| DaemonError::Internal(format!("blocking task panicked: {e}")))
}

/// Like [`spawn_query`], for a synchronous body that can itself fail (the
/// hygiene write arms, which surface a `DaemonError` from the store call).
async fn spawn_query_result<F>(f: F) -> Result<Value, DaemonError>
where
    F: FnOnce() -> Result<Value, DaemonError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| DaemonError::Internal(format!("blocking task panicked: {e}")))?
}

/// A store-seeded collection ready to share: the collection doc (manifest +
/// owner/policy) plus each node's `(node_id, encoded yrs state)`.
type SeededCollection = (mae_sync::kb::KbCollectionDoc, Vec<(String, Vec<u8>)>);

/// Resolve the CozoDB store backing `kb_id` (a KB *name*): the primary KB's own
/// store, or a named instance's store. `None` when the name isn't registered with
/// this daemon — the share still proceeds at collection level, just without seeded
/// node content.
pub(crate) fn resolve_kb_store(st: &DaemonState, kb_id: &str) -> Option<Arc<CozoKbStore>> {
    let inst = st.registry.find(kb_id)?;
    if inst.primary {
        st.store.clone()
    } else {
        st.instance_stores.get(&inst.uuid).cloned()
    }
}

/// ADR-061 Phase D3: the production `mae_daemon::artifact_store::ArtifactStore` —
/// bridges `collab_handler`'s `kb/fetch_artifact` RPC (library crate) to this
/// binary crate's `DaemonState`/`resolve_kb_store`, mirroring
/// `collab_handler::kb_lease::DaemonLeaseFence`'s identical crate-boundary
/// pattern from Phase D2.
pub struct DaemonArtifactStore(pub Arc<Mutex<DaemonState>>);

#[async_trait::async_trait]
impl mae_daemon::artifact_store::ArtifactStore for DaemonArtifactStore {
    async fn get_cached_embedding(
        &self,
        kb_id: &str,
        content_hash: &str,
        model: &str,
        chunk_version: i64,
    ) -> Result<Option<Vec<f32>>, String> {
        let store = {
            let st = self.0.lock().await;
            resolve_kb_store(&st, kb_id)
                .ok_or_else(|| format!("KB '{kb_id}' has no local content store on this daemon"))?
        };
        let content_hash = content_hash.to_string();
        let model = model.to_string();
        // ADR-054: a synchronous CozoDB read must not run inline on the async
        // executor, matching every other store read in this daemon.
        tokio::task::spawn_blocking(move || {
            store
                .get_cached_embedding(&content_hash, &model, chunk_version)
                .map_err(|e| format!("cache lookup failed: {e}"))
        })
        .await
        .map_err(|e| format!("cache lookup task panicked: {e}"))?
    }
}

/// Establish (or widen) a P2P mesh share for `kb_id` directly via the control
/// socket — the daemon's self-sufficient `kb-share-p2p` path (ADR-025). On a FRESH
/// share it creates the `kbc:{kb_id}` collection owned by this daemon, **seeds its
/// node docs** (`seed` = the collection + node states built from the daemon KB
/// store, byte-identical to the editor's `kb/share`), and exposes it on the mesh;
/// on a re-share it widens the existing collection's transport policy (+ optional
/// join policy) WITHOUT clobbering daemon-side membership or nodes (B-12). Returns
/// `(created, node_count)`.
async fn establish_p2p_share(
    doc_store: &Arc<mae_daemon::doc_store::DocStore>,
    broadcaster: &mae_mcp::broadcast::SharedBroadcaster,
    owner: &mae_mcp::identity::Identity,
    kb_id: &str,
    transport: mae_sync::kb::TransportPolicy,
    policy: Option<mae_sync::kb::JoinPolicy>,
    seed: Option<SeededCollection>,
) -> Result<(bool, usize), DaemonError> {
    let owner_fp = owner.fingerprint();
    let collection_doc = format!("kbc:{kb_id}");

    // Persist a collection update + broadcast it to any subscribed sync session
    // (parity with the TCP `kb/share` persist+broadcast).
    async fn persist(
        doc_store: &mae_daemon::doc_store::DocStore,
        broadcaster: &mae_mcp::broadcast::SharedBroadcaster,
        collection_doc: &str,
        update: &[u8],
    ) -> Result<(), DaemonError> {
        let result = doc_store
            .apply_update(collection_doc, update, None)
            .await
            .map_err(|e| DaemonError::Internal(format!("persist collection: {e}")))?;
        broadcaster
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .broadcast(&mae_mcp::broadcast::EditorEvent::SyncUpdate {
                buffer_name: collection_doc.to_string(),
                update_base64: mae_sync::encoding::update_to_base64(update),
                wal_seq: result.wal_seq,
                content_header: None,
            });
        Ok(())
    }

    if doc_store.has_doc(&collection_doc).await {
        // Existing collection (B-12: never clobber daemon-side membership or nodes)
        // — widen transport to include the mesh + optionally adjust the join policy.
        let (state_bytes, _sv) = doc_store
            .encode_state_and_sv(&collection_doc)
            .await
            .map_err(|e| DaemonError::Internal(format!("load collection: {e}")))?;
        let mut coll = mae_sync::kb::KbCollectionDoc::from_bytes(&state_bytes)
            .map_err(|e| DaemonError::Internal(format!("bad collection: {e}")))?;
        let raw = coll.transport_policy_raw();
        let widened = raw.map_or(transport, |c| c.union(transport));
        if Some(widened) != raw {
            let update = coll.set_transport_policy(widened);
            persist(doc_store, broadcaster, &collection_doc, &update).await?;
        }
        if let Some(p) = policy {
            if coll.join_policy() != p {
                let update = coll.set_join_policy(p);
                persist(doc_store, broadcaster, &collection_doc, &update).await?;
            }
        }
        Ok((false, coll.list_nodes().len()))
    } else {
        // Fresh collection owned by this daemon, exposed on the mesh. Start from the
        // store-seeded collection (with its node manifest already populated by
        // `to_collection`) when available, else an empty one.
        let (mut coll, node_states) = seed.unwrap_or_else(|| {
            (
                mae_sync::kb::KbCollectionDoc::new(kb_id, &owner_fp),
                Vec::new(),
            )
        });
        coll.set_owner(&owner_fp, owner.label());
        coll.set_transport_policy(transport);
        if let Some(p) = policy {
            coll.set_join_policy(p);
        }
        // ADR-043: seed the SIGNED owner-genesis so a fresh mesh share anchors membership + E2E
        // key-derivation identically to the hub `kb/share` path (collab_handler.rs "Seed the
        // genesis owner self-admit"). Without it the collection is roster-only — not
        // peer-verifiable and not E2E-capable (`derive_valid_members` / `find_wrapped_content_key`
        // have no anchor). Closes the #237 p2p-no-genesis gap. `to_collection` / `new` produce an
        // empty op-log, so this always fires on a fresh share.
        if coll.oplog_head().is_none() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let g = coll.build_membership_op(
                kb_id,
                mae_sync::membership::MembershipAction::Admit,
                &owner_fp,
                Some(mae_sync::kb::Role::Owner),
                true,
                &owner_fp,
                now,
                None,
                0,
            );
            let gsig = g.sign(&owner.secret_bytes());
            coll.append_signed_op(&g, &gsig, &owner.public().to_bytes());
        }
        doc_store
            .share_doc(&collection_doc, &coll.encode_state())
            .await
            .map_err(|e| DaemonError::Internal(format!("share collection: {e}")))?;
        // Seed each node doc so a joining peer pulls real content, not just the
        // manifest. Same naming + encoding as the TCP `kb/share` path.
        for (node_id, state) in &node_states {
            let node_doc = mae_sync::kb_node_doc_name(kb_id, node_id);
            let res = if doc_store.has_doc(&node_doc).await {
                doc_store
                    .apply_update(&node_doc, state, None)
                    .await
                    .map(|_| ())
            } else {
                doc_store.share_doc(&node_doc, state).await.map(|_| ())
            };
            res.map_err(|e| DaemonError::Internal(format!("seed node '{node_id}': {e}")))?;
        }
        Ok((true, node_states.len()))
    }
}

/// Daemon-specific errors.
#[derive(Debug)]
pub enum DaemonError {
    InvalidParams(&'static str),
    NotReady,
    MethodNotFound(String),
    Internal(String),
    /// ADR-060 Phase A: an RPC's `instance` address (name or UUID) didn't
    /// resolve to any instance this daemon has registered. Distinct from
    /// `InvalidParams` (the address is well-formed, just unknown) so a
    /// client can tell "you typo'd the address" apart from "malformed
    /// request" — same JSON-RPC code today, but a distinguishable variant
    /// for future per-tenant error handling (Phase C/D).
    UnknownInstance(String),
    /// ADR-060 Phase C: the resolved tenant's cost-weighted points budget
    /// for the current 60s window is exhausted. Distinct from
    /// `TenantAtCapacity` (a different resource dimension — see the ADR's
    /// Decision section on why they're two independent caps, not one).
    QuotaExceeded(String),
    /// ADR-060 Phase C: the resolved tenant already has `max_connections`
    /// requests in flight.
    TenantAtCapacity(String),
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonError::InvalidParams(msg) => write!(f, "Invalid params: {msg}"),
            DaemonError::NotReady => write!(f, "Daemon not ready (no KB store loaded)"),
            DaemonError::MethodNotFound(m) => write!(f, "Method not found: {m}"),
            DaemonError::Internal(msg) => write!(f, "Internal error: {msg}"),
            DaemonError::UnknownInstance(addr) => {
                write!(f, "Unknown instance address: {addr}")
            }
            DaemonError::QuotaExceeded(tenant) => {
                write!(
                    f,
                    "Tenant '{tenant}' has exhausted its request budget for this window"
                )
            }
            DaemonError::TenantAtCapacity(tenant) => {
                write!(
                    f,
                    "Tenant '{tenant}' has reached its concurrent-request cap"
                )
            }
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
            DaemonError::UnknownInstance(_) => -32602,
            // Server-error range (-32000..-32099), not one of the standard
            // JSON-RPC codes above -- a rate-limit-shaped rejection is
            // neither a malformed request nor an internal failure.
            DaemonError::QuotaExceeded(_) => -32001,
            DaemonError::TenantAtCapacity(_) => -32002,
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
        // Version is reported for the editor's version-skew check (ADR-035).
        assert_eq!(
            result["version"].as_str(),
            Some(env!("CARGO_PKG_VERSION")),
            "daemon/status must report the daemon version"
        );
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch("nonexistent/method", json!({}), &state).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn neighborhood_and_related_without_store_are_not_ready() {
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let n = dispatch("kb/neighborhood", json!({"id": "concept:x"}), &state).await;
        assert!(matches!(n, Err(DaemonError::NotReady)));
        let r = dispatch("kb/related", json!({"id": "concept:x"}), &state).await;
        assert!(matches!(r, Err(DaemonError::NotReady)));
        let t = dispatch("kb/todo_nodes", json!({}), &state).await;
        assert!(matches!(t, Err(DaemonError::NotReady)));
    }

    #[tokio::test]
    async fn todo_nodes_rpc_serves_todo_set_without_crdt_doc() {
        // A store with a TODO node, a DONE node, and a plain note.
        let store = mae_kb::CozoKbStore::open_mem().unwrap();
        store
            .insert_node(
                &mae_kb::Node::new("task:a", "Do A", mae_kb::NodeKind::Task, "body")
                    .with_todo_state("TODO"),
            )
            .unwrap();
        store
            .insert_node(
                &mae_kb::Node::new("task:b", "Do B", mae_kb::NodeKind::Task, "")
                    .with_todo_state("DONE"),
            )
            .unwrap();
        store
            .insert_node(&mae_kb::Node::new(
                "note:c",
                "Plain",
                mae_kb::NodeKind::Note,
                "",
            ))
            .unwrap();

        let mut st = DaemonState::new();
        st.store = Some(Arc::new(store));
        st.rebuild_query_layer();
        let state = Arc::new(Mutex::new(st));

        let r = dispatch("kb/todo_nodes", json!({}), &state).await.unwrap();
        let arr = r.as_array().expect("todo_nodes returns a JSON array");
        let ids: Vec<&str> = arr.iter().filter_map(|n| n["id"].as_str()).collect();
        assert!(ids.contains(&"task:a"), "TODO node present: {ids:?}");
        assert!(ids.contains(&"task:b"), "DONE node present: {ids:?}");
        assert!(!ids.contains(&"note:c"), "plain note excluded: {ids:?}");
        // The heavy lineage is stripped to keep the payload lean.
        for n in arr {
            assert!(
                n.get("crdt_doc").is_none_or(|v| v.is_null()),
                "crdt_doc must be cleared in the RPC payload: {n}"
            );
        }
    }

    #[tokio::test]
    async fn status_reports_no_collections_without_doc_store() {
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let r = dispatch("daemon/status", json!({}), &state).await.unwrap();
        assert_eq!(r["primary_exists"].as_bool(), Some(false));
        assert_eq!(r["kb_collections"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn kb_node_crdt_returns_state_for_hosted_node_else_null() {
        use mae_daemon::doc_store::DocStore;
        use mae_daemon::storage::SqliteBackend;
        let ds = Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open_memory().unwrap()),
            500,
        ));
        let node = mae_sync::kb::KbNodeDoc::new("concept:x", "X", "body", &[]);
        ds.share_doc("kbn:testkb:concept:x", &node.encode())
            .await
            .unwrap();
        let mut st = DaemonState::new();
        st.doc_store = Some(ds);
        let state = Arc::new(Mutex::new(st));

        // Hosted node → base64 CRDT state.
        let r = dispatch(
            "kb/node_crdt",
            json!({"kb_id": "testkb", "id": "concept:x"}),
            &state,
        )
        .await
        .unwrap();
        assert!(
            r["state"].as_str().is_some(),
            "hosted node must return CRDT state: {r}"
        );
        // Absent node → null (no spurious empty-doc materialization).
        let r2 = dispatch(
            "kb/node_crdt",
            json!({"kb_id": "testkb", "id": "concept:absent"}),
            &state,
        )
        .await
        .unwrap();
        assert!(r2.is_null(), "absent node must return null, got: {r2}");
    }

    #[tokio::test]
    async fn status_reports_hosted_collections_and_primary() {
        use mae_daemon::doc_store::DocStore;
        use mae_daemon::storage::SqliteBackend;
        let ds = Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open_memory().unwrap()),
            500,
        ));
        let c1 = mae_sync::kb::KbCollectionDoc::new("default", "owner");
        ds.share_doc("kbc:default", &c1.encode_state())
            .await
            .unwrap();
        let c2 = mae_sync::kb::KbCollectionDoc::new("notes", "owner");
        ds.share_doc("kbc:notes", &c2.encode_state()).await.unwrap();

        let mut st = DaemonState::new();
        st.doc_store = Some(ds);
        let state = Arc::new(Mutex::new(st));

        let r = dispatch("daemon/status", json!({}), &state).await.unwrap();
        assert_eq!(
            r["primary_exists"].as_bool(),
            Some(true),
            "kbc:default ⇒ primary_exists"
        );
        let cols: Vec<String> = r["kb_collections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            cols.contains(&"default".to_string()) && cols.contains(&"notes".to_string()),
            "got: {cols:?}"
        );
    }

    #[tokio::test]
    async fn kb_get_without_store_returns_not_ready() {
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch("kb/get", json!({"id": "test:node"}), &state).await;
        assert!(matches!(result, Err(DaemonError::NotReady)));
    }

    #[tokio::test]
    async fn mint_ticket_without_mesh_is_not_ready() {
        // P2P disabled (no endpoint) → the control method reports NotReady rather
        // than minting a useless ticket.
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch("p2p/mint_ticket", json!({"kb_id": "kbx"}), &state).await;
        assert!(matches!(result, Err(DaemonError::NotReady)));
    }

    #[tokio::test]
    async fn mint_ticket_requires_kb_id() {
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch("p2p/mint_ticket", json!({}), &state).await;
        assert!(matches!(result, Err(DaemonError::InvalidParams(_))));
    }

    #[tokio::test]
    async fn mint_ticket_with_mesh_returns_a_join_link() {
        // With a bound mesh endpoint, the control method returns a parseable
        // `mae://join/` ticket carrying the requested KB-id.
        let id = mae_mcp::identity::Identity::generate("owner");
        let endpoint = crate::p2p::bind_endpoint(&id, iroh::RelayMode::Disabled)
            .await
            .unwrap();
        let mut st = DaemonState::new();
        st.p2p_endpoint = Some(endpoint.clone());
        let state = Arc::new(Mutex::new(st));

        let result = dispatch("p2p/mint_ticket", json!({"kb_id": "concept:x"}), &state)
            .await
            .unwrap();
        let ticket = result["ticket"].as_str().expect("ticket string");
        assert!(ticket.starts_with("mae://join/"), "got: {ticket}");
        assert_eq!(result["kb_id"].as_str(), Some("concept:x"));
        let parsed: crate::ticket::JoinTicket = ticket.parse().expect("ticket re-parses");
        assert_eq!(parsed.kb_id, "concept:x");

        endpoint.close().await;
    }

    #[tokio::test]
    async fn join_ticket_records_a_pending_target_idempotently() {
        // A real minted ticket round-trips through the join method. This daemon's
        // OWN mesh must be running to accept the join (ADR-086 below is the
        // no-mesh counterpart) -- `run_dialer`, the only consumer of
        // `pending_p2p_joins`, is spawned alongside a bound endpoint, so the join
        // target used here is a SEPARATE peer's ticket, not this daemon's own.
        let id = mae_mcp::identity::Identity::generate("owner");
        let remote_endpoint = crate::p2p::bind_endpoint(&id, iroh::RelayMode::Disabled)
            .await
            .unwrap();
        let ticket = crate::p2p::mint_ticket(&remote_endpoint, "concept:x").to_string();
        remote_endpoint.close().await;

        let local_id = mae_mcp::identity::Identity::generate("local");
        let local_endpoint = crate::p2p::bind_endpoint(&local_id, iroh::RelayMode::Disabled)
            .await
            .unwrap();
        let mut st = DaemonState::new();
        st.p2p_endpoint = Some(local_endpoint.clone());
        let state = Arc::new(Mutex::new(st));

        let result = dispatch("p2p/join_ticket", json!({ "ticket": ticket }), &state)
            .await
            .unwrap();
        assert_eq!(result["kb_id"].as_str(), Some("concept:x"));
        assert_eq!(result["status"].as_str(), Some("recorded"));
        assert!(result["peer"].as_str().unwrap().starts_with("SHA256:"));
        assert_eq!(state.lock().await.pending_p2p_joins.len(), 1);

        // Re-accepting the same ticket does not double-queue (ADR-086 D2: a
        // repeat request against an already-satisfied/queued state is not an
        // error).
        dispatch("p2p/join_ticket", json!({ "ticket": ticket }), &state)
            .await
            .unwrap();
        assert_eq!(state.lock().await.pending_p2p_joins.len(), 1);

        local_endpoint.close().await;
    }

    /// ADR-086: with no P2P endpoint bound, `run_dialer` (the only consumer of
    /// `pending_p2p_joins`) never gets spawned (see `main.rs`), so queuing a
    /// join here would sit forever and the "recorded, will connect" reply
    /// would describe a join that can never happen. The requested
    /// postcondition (a join actually in flight to be dialed) does not hold,
    /// so this must be `Err`, and the ticket must NOT be queued.
    #[tokio::test]
    async fn join_ticket_without_mesh_is_not_ready() {
        let id = mae_mcp::identity::Identity::generate("owner");
        let endpoint = crate::p2p::bind_endpoint(&id, iroh::RelayMode::Disabled)
            .await
            .unwrap();
        let ticket = crate::p2p::mint_ticket(&endpoint, "concept:x").to_string();
        endpoint.close().await;

        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch("p2p/join_ticket", json!({ "ticket": ticket }), &state).await;
        assert!(
            matches!(result, Err(DaemonError::NotReady)),
            "expected NotReady with no mesh running, got {result:?}"
        );
        assert!(
            state.lock().await.pending_p2p_joins.is_empty(),
            "a refused join must not be queued as if it would be serviced"
        );
    }

    #[tokio::test]
    async fn join_ticket_rejects_a_malformed_ticket() {
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch(
            "p2p/join_ticket",
            json!({ "ticket": "not-a-ticket" }),
            &state,
        )
        .await;
        assert!(matches!(result, Err(DaemonError::InvalidParams(_))));
        assert!(state.lock().await.pending_p2p_joins.is_empty());
    }

    /// Build a `DaemonState` wired for `p2p/share_kb`: an in-memory doc_store, a
    /// broadcaster, and an owner identity (mirrors `spawn_collab_server`).
    fn share_kb_state() -> (Arc<Mutex<DaemonState>>, Arc<mae_mcp::identity::Identity>) {
        let backend = Arc::new(mae_daemon::storage::SqliteBackend::open_memory().unwrap());
        let doc_store = Arc::new(mae_daemon::doc_store::DocStore::new(backend, 0));
        let owner = Arc::new(mae_mcp::identity::Identity::generate("daemon"));
        let broadcaster: mae_mcp::broadcast::SharedBroadcaster = Arc::new(std::sync::Mutex::new(
            mae_mcp::broadcast::EventBroadcaster::new(),
        ));
        let mut st = DaemonState::new();
        st.doc_store = Some(doc_store);
        st.broadcaster = Some(broadcaster);
        st.owner = Some(Arc::clone(&owner));
        (Arc::new(Mutex::new(st)), owner)
    }

    #[tokio::test]
    async fn share_kb_requires_kb_id() {
        let (state, _owner) = share_kb_state();
        let result = dispatch("p2p/share_kb", json!({}), &state).await;
        assert!(matches!(result, Err(DaemonError::InvalidParams(_))));
    }

    #[tokio::test]
    async fn share_kb_without_collab_is_an_error() {
        // No doc_store/owner wired (collab off or non-key mode) → actionable error,
        // never a silent success.
        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch("p2p/share_kb", json!({"kb_id": "concept:x"}), &state).await;
        assert!(matches!(result, Err(DaemonError::Internal(_))));
    }

    #[tokio::test]
    async fn share_kb_creates_a_mesh_collection_then_widens() {
        use mae_sync::kb::{JoinPolicy, KbCollectionDoc, Transport, TransportPolicy};
        let (state, owner) = share_kb_state();

        // First share: creates the collection, owned by this daemon, on the mesh.
        let result = dispatch(
            "p2p/share_kb",
            json!({"kb_id": "concept:x", "policy": "permissive"}),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(result["created"].as_bool(), Some(true));
        assert_eq!(result["transport"].as_str(), Some("p2p"));
        assert_eq!(result["status"].as_str(), Some("shared"));

        // The collection now exists with owner = this daemon, P2p exposure, and the
        // requested permissive join policy — so a mesh peer can actually pull it.
        let doc_store = state.lock().await.doc_store.clone().unwrap();
        let (bytes, _sv) = doc_store
            .encode_state_and_sv("kbc:concept:x")
            .await
            .unwrap();
        let coll = KbCollectionDoc::from_bytes(&bytes).unwrap();
        assert_eq!(coll.owner(), owner.fingerprint());
        assert!(coll.transport_policy().allows(Transport::P2p));
        assert_eq!(coll.join_policy(), JoinPolicy::Permissive);

        // Re-share as hub → widens to Both (the mesh exposure is preserved, B-12).
        let result = dispatch(
            "p2p/share_kb",
            json!({"kb_id": "concept:x", "transport": "hub"}),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(result["created"].as_bool(), Some(false));
        let (bytes, _sv) = doc_store
            .encode_state_and_sv("kbc:concept:x")
            .await
            .unwrap();
        let coll = KbCollectionDoc::from_bytes(&bytes).unwrap();
        assert_eq!(coll.transport_policy(), TransportPolicy::Both);
        assert!(coll.transport_policy().allows(Transport::P2p));
        assert!(coll.transport_policy().allows(Transport::Hub));
    }

    /// ADR-043 (#237, #182) — a fresh P2P mesh share must seed a SIGNED owner-genesis, not a
    /// roster-only manifest, so the collection is peer-verifiable + E2E key-derivation capable
    /// (the whole reason E2E-on-mesh was previously impossible). Proves the owner derives from
    /// the signed op-log anchored on its own key.
    #[tokio::test]
    async fn share_kb_seeds_a_signed_owner_genesis_so_the_collection_is_anchorable() {
        use mae_sync::kb::{KbCollectionDoc, Role};
        use mae_sync::membership::derive_valid_members;
        let (state, owner) = share_kb_state();
        dispatch(
            "p2p/share_kb",
            json!({"kb_id": "concept:x", "policy": "permissive"}),
            &state,
        )
        .await
        .unwrap();
        let doc_store = state.lock().await.doc_store.clone().unwrap();
        let (bytes, _sv) = doc_store
            .encode_state_and_sv("kbc:concept:x")
            .await
            .unwrap();
        let coll = KbCollectionDoc::from_bytes(&bytes).unwrap();

        // The share carries a signed op-log genesis (not roster-only).
        assert!(
            coll.oplog_head().is_some(),
            "a fresh mesh share must seed a signed op-log genesis"
        );
        // The genesis anchors membership derivation on the owner's key — so the collection is
        // peer-verifiable (ADR-026) and E2E-anchorable (ADR-037 `find_wrapped_content_key`).
        let members =
            derive_valid_members(&coll.oplog_ops(), &owner.public().to_bytes(), 9_999_999_999);
        assert_eq!(
            members.get(&owner.fingerprint()).map(|m| m.role),
            Some(Role::Owner),
            "the seeded genesis makes the owner derivable as Owner from the signed op-log"
        );
    }

    #[tokio::test]
    async fn share_kb_seeds_node_content_from_the_store() {
        use mae_sync::kb::{KbCollectionDoc, KbNodeDoc};
        // An in-memory KB store holding one node with real content.
        let store = mae_kb::CozoKbStore::open_mem().unwrap();
        let mut node = mae_kb::Node::new(
            "collabtest:overview",
            "Overview",
            mae_kb::NodeKind::Concept,
            "the ZEPHYRINE protocol",
        );
        node.tags = vec!["alpha".to_string()];
        store.insert_node(&node).unwrap();

        // Wire that store into the daemon state as the primary KB named "collabtest".
        let (state, _owner) = share_kb_state();
        {
            let mut st = state.lock().await;
            st.store = Some(Arc::new(store));
            st.registry.instances.push(mae_kb::federation::KbInstance {
                uuid: "u1".to_string(),
                name: "collabtest".to_string(),
                org_dir: std::path::PathBuf::new(),
                db_path: std::path::PathBuf::new(),
                primary: true,
                enabled: true,
                last_import: None,
                collab_id: None,
                shared: false,
                remote_peers: Vec::new(),
                last_sync: None,
                ai_residency: mae_kb::federation::AiResidency::default(),
                project_root: None,
                kind: mae_kb::federation::KbInstanceKind::default(),
                ingest_policy: Default::default(),
                priority: 0,
                remote_hub: None,
            });
        }

        let result = dispatch(
            "p2p/share_kb",
            json!({"kb_id": "collabtest", "policy": "permissive"}),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(result["created"].as_bool(), Some(true));
        assert_eq!(
            result["nodes"].as_u64(),
            Some(1),
            "the node should be seeded"
        );

        // The collection manifest lists the node; the node doc carries the content
        // (so a joining peer pulls real content, not just the manifest).
        let doc_store = state.lock().await.doc_store.clone().unwrap();
        let (cbytes, _) = doc_store
            .encode_state_and_sv("kbc:collabtest")
            .await
            .unwrap();
        assert_eq!(
            KbCollectionDoc::from_bytes(&cbytes)
                .unwrap()
                .list_nodes()
                .len(),
            1
        );
        let (nbytes, _) = doc_store
            .encode_state_and_sv(&mae_sync::kb_node_doc_name(
                "collabtest",
                "collabtest:overview",
            ))
            .await
            .unwrap();
        let node_doc = KbNodeDoc::from_bytes(&nbytes).unwrap();
        assert!(
            node_doc.body().contains("ZEPHYRINE"),
            "seeded node doc must carry the body content"
        );
    }

    // ---- ADR-060 Phase A: per-tenant RPC addressing ----

    /// Build a `DaemonState` with 3 registered instances (principle #14: ≥3 when
    /// isolation is the property under test, not 2) — a primary and two named
    /// secondaries — each holding a node under the SAME id but with distinct,
    /// real content, so a test can prove a targeted query returns exactly one
    /// tenant's data and never another's (a selective oracle on content, not
    /// merely "something was found").
    fn three_instance_state() -> (Arc<Mutex<DaemonState>>, String, String, String) {
        let primary_store = mae_kb::CozoKbStore::open_mem().unwrap();
        primary_store
            .insert_node(&mae_kb::Node::new(
                "shared-id",
                "Team A's note",
                mae_kb::NodeKind::Note,
                "TEAM A CONTENT",
            ))
            .unwrap();

        let team_b_store = mae_kb::CozoKbStore::open_mem().unwrap();
        team_b_store
            .insert_node(&mae_kb::Node::new(
                "shared-id",
                "Team B's note",
                mae_kb::NodeKind::Note,
                "TEAM B CONTENT",
            ))
            .unwrap();

        let team_c_store = mae_kb::CozoKbStore::open_mem().unwrap();
        team_c_store
            .insert_node(&mae_kb::Node::new(
                "shared-id",
                "Team C's note",
                mae_kb::NodeKind::Note,
                "TEAM C CONTENT",
            ))
            .unwrap();

        let uuid_a = "uuid-team-a".to_string();
        let uuid_b = "uuid-team-b".to_string();
        let uuid_c = "uuid-team-c".to_string();

        let mut st = DaemonState::new();
        st.store = Some(Arc::new(primary_store));
        st.instance_stores
            .insert(uuid_b.clone(), Arc::new(team_b_store));
        st.instance_stores
            .insert(uuid_c.clone(), Arc::new(team_c_store));

        let mk_instance = |uuid: &str, name: &str, primary: bool| mae_kb::federation::KbInstance {
            uuid: uuid.to_string(),
            name: name.to_string(),
            org_dir: std::path::PathBuf::new(),
            db_path: std::path::PathBuf::new(),
            primary,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
            project_root: None,
            kind: mae_kb::federation::KbInstanceKind::default(),
            ingest_policy: Default::default(),
            priority: 0,
            remote_hub: None,
        };
        st.registry
            .instances
            .push(mk_instance(&uuid_a, "team-a", true));
        st.registry
            .instances
            .push(mk_instance(&uuid_b, "team-b", false));
        st.registry
            .instances
            .push(mk_instance(&uuid_c, "team-c", false));
        st.rebuild_query_layer();

        (Arc::new(Mutex::new(st)), uuid_a, uuid_b, uuid_c)
    }

    #[tokio::test]
    async fn instance_addr_omitted_preserves_todays_federated_behavior() {
        // Backward compatibility is load-bearing (ADR-060 Phase A): omitting
        // `instance` must behave exactly as before this ADR -- the federated
        // layer across every registered instance, unchanged.
        let (state, ..) = three_instance_state();
        let r = dispatch("kb/get", json!({"id": "shared-id"}), &state)
            .await
            .unwrap();
        // The federated layer resolves the collision by priority/order, but the
        // key regression-guard property is simply that omitting the address
        // still finds *a* result via the pre-existing federated path, not an
        // UnknownInstance error or a NotReady failure.
        assert!(
            !r.is_null(),
            "omitted instance must still resolve via federation"
        );
    }

    #[tokio::test]
    async fn instance_addr_scopes_strictly_to_the_addressed_tenant_never_another() {
        // The core Phase A isolation property: addressing team B's instance for a
        // node id that ALSO exists (with different content) in team A and team C
        // must return exactly team B's content -- never A's, never C's, and never
        // a federated merge of any of them.
        let (state, uuid_a, uuid_b, uuid_c) = three_instance_state();

        let a = dispatch(
            "kb/get",
            json!({"id": "shared-id", "instance": uuid_a}),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(a["body"].as_str(), Some("TEAM A CONTENT"));

        let b = dispatch(
            "kb/get",
            json!({"id": "shared-id", "instance": uuid_b}),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(
            b["body"].as_str(),
            Some("TEAM B CONTENT"),
            "addressing team B must never return team A's or team C's content"
        );

        let c = dispatch(
            "kb/get",
            json!({"id": "shared-id", "instance": uuid_c}),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(
            c["body"].as_str(),
            Some("TEAM C CONTENT"),
            "addressing team C must never return team A's or team B's content"
        );
    }

    #[tokio::test]
    async fn instance_addr_accepts_either_name_or_uuid_for_the_same_instance() {
        // resolve_kb_store (reused, not reinvented -- principle #8) already
        // resolves by name OR uuid; Phase A's addressing must expose both, since
        // that's the address space it claims to reuse.
        let (state, _uuid_a, uuid_b, _uuid_c) = three_instance_state();

        let by_uuid = dispatch(
            "kb/get",
            json!({"id": "shared-id", "instance": uuid_b}),
            &state,
        )
        .await
        .unwrap();
        let by_name = dispatch(
            "kb/get",
            json!({"id": "shared-id", "instance": "team-b"}),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(by_uuid["body"], by_name["body"]);
        assert_eq!(by_name["body"].as_str(), Some("TEAM B CONTENT"));
    }

    #[tokio::test]
    async fn instance_addr_unknown_is_a_clean_error_not_a_silent_fallback_or_panic() {
        // An address that doesn't resolve to any registered instance must fail
        // explicitly -- never silently fall through to the primary/federated
        // result (which would be a real cross-tenant data leak if this were a
        // typo'd or forged address in a real deployment) and never panic.
        let (state, ..) = three_instance_state();
        let r = dispatch(
            "kb/get",
            json!({"id": "shared-id", "instance": "no-such-tenant"}),
            &state,
        )
        .await;
        match r {
            Err(DaemonError::UnknownInstance(addr)) => assert_eq!(addr, "no-such-tenant"),
            other => panic!("expected UnknownInstance, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn instance_addr_search_is_also_scoped_not_only_get() {
        // Addressing must apply uniformly across the query-layer arms, not just
        // kb/get -- kb/search must also never surface another tenant's hits.
        let (state, _uuid_a, uuid_b, _uuid_c) = three_instance_state();
        let r = dispatch(
            "kb/search",
            json!({"query": "TEAM", "instance": uuid_b}),
            &state,
        )
        .await
        .unwrap();
        let hits = r.as_array().unwrap();
        assert_eq!(
            hits.len(),
            1,
            "team B's addressed store has exactly one matching node: {hits:?}"
        );
        assert_eq!(hits[0]["id"].as_str(), Some("shared-id"));
    }

    #[tokio::test]
    async fn instance_addr_also_scopes_the_store_based_hygiene_arms() {
        // snapshot_store (the hygiene arms' equivalent of snapshot_query_layer)
        // must honor the same addressing -- Phase A's DoD is "every daemon RPC",
        // not just the query-layer ones.
        let (state, uuid_a, uuid_b, _uuid_c) = three_instance_state();

        let scan_a = dispatch("kb/hygiene_scan", json!({"instance": uuid_a}), &state)
            .await
            .unwrap();
        let scan_b = dispatch("kb/hygiene_scan", json!({"instance": uuid_b}), &state)
            .await
            .unwrap();
        // Both must succeed independently against their own addressed store
        // (not error, not silently reuse the primary's scan) -- the specific
        // hygiene findings aren't the point here, that both resolve distinctly is.
        assert!(scan_a.is_object() || scan_a.is_array());
        assert!(scan_b.is_object() || scan_b.is_array());

        let unknown = dispatch(
            "kb/hygiene_scan",
            json!({"instance": "no-such-tenant"}),
            &state,
        )
        .await;
        assert!(matches!(unknown, Err(DaemonError::UnknownInstance(_))));
    }

    // ---- ADR-060 Phase D: the IDOR-shaped adversarial case ----
    //
    // Named as this ADR's own single highest-priority adversarial test, per the
    // real Gitea (CVE-2026-27771/CVE-2026-58444) and Vaultwarden (CVE-2026-27898)
    // precedent cited in the ADR's Context: a request correctly, validly
    // addressed at tenant A's own instance whose payload separately references a
    // raw ID that actually belongs to a DIFFERENT tenant's data must be rejected
    // at ID-resolution time -- not served just because the outer address was
    // fine. Written and run against the current (post-Phase-A) code first, per
    // the same principle-#15 discipline that resolved Phase B, before assuming
    // any new resolution-time check needs to be built.

    #[tokio::test]
    async fn idor_a_valid_instance_address_never_resolves_a_different_tenants_id() {
        let (state, uuid_a, uuid_b, uuid_c) = three_instance_state();

        // A node ID that exists ONLY in tenant B's store, never in A's or C's --
        // the exact IDOR shape: address A validly, but ask for an ID that lives
        // in B.
        {
            let st = state.lock().await;
            let b_store = st.instance_stores.get(&uuid_b).unwrap();
            b_store
                .insert_node(&mae_kb::Node::new(
                    "b-only-secret",
                    "Team B's secret note",
                    mae_kb::NodeKind::Note,
                    "TEAM B SECRET CONTENT -- must never be reachable via tenant A's address",
                ))
                .unwrap();
        }

        // kb/get addressed at A, requesting the ID that only exists in B.
        let r = dispatch(
            "kb/get",
            json!({"id": "b-only-secret", "instance": uuid_a.clone()}),
            &state,
        )
        .await
        .unwrap();
        assert!(
            r.is_null(),
            "an ID that only exists in tenant B must be Null (not found), never resolved, \
             when the request is addressed at tenant A: {r:?}"
        );

        // Same shape via kb/links_from/kb/links_to -- the other id-taking arms
        // that resolve a raw, request-supplied identifier against the addressed
        // store, not just kb/get.
        let links_from = dispatch(
            "kb/links_from",
            json!({"id": "b-only-secret", "instance": uuid_a.clone()}),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(
            links_from.as_array().map(|a| a.len()),
            Some(0),
            "links_from for a B-only id addressed at A must be empty, not B's real links"
        );
        let links_to = dispatch(
            "kb/links_to",
            json!({"id": "b-only-secret", "instance": uuid_a}),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(
            links_to.as_array().map(|a| a.len()),
            Some(0),
            "links_to for a B-only id addressed at A must be empty, not B's real links"
        );

        // Sanity: the SAME id, addressed correctly at B, DOES resolve -- proving
        // the null above is genuine cross-tenant isolation, not a broken lookup.
        let via_b = dispatch(
            "kb/get",
            json!({"id": "b-only-secret", "instance": uuid_b}),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(
            via_b["body"].as_str(),
            Some("TEAM B SECRET CONTENT -- must never be reachable via tenant A's address"),
            "the id genuinely exists and is reachable when addressed at its real tenant B"
        );

        // And tenant C (a third, uninvolved tenant, per principle #14's N-way
        // requirement) must ALSO never see B's id when addressed at C.
        let via_c = dispatch(
            "kb/get",
            json!({"id": "b-only-secret", "instance": uuid_c}),
            &state,
        )
        .await
        .unwrap();
        assert!(
            via_c.is_null(),
            "a B-only id addressed at an uninvolved third tenant C must also be Null: {via_c:?}"
        );
    }

    // ---- ADR-060 Phase B: N-way concurrency isolation ----
    //
    // This is the test ADR-060's own Verification section names as "the highest-
    // priority test alongside Phase D's IDOR case" -- and, per the ADR's own
    // instruction to re-check the literal mechanism against reality before
    // implementing (the same discipline ADR-054's Implementation Note applied),
    // it is written and run FIRST, against the current Phase-A-only code, before
    // any lock-splitting rewrite. ADR-054 already generalized "snapshot-then-
    // drop" (clone the needed Arc under the lock, drop the lock, do the real
    // work in spawn_blocking) to every read/hygiene arm in this file AND to
    // scheduler.rs's watcher/maintenance/health ticks -- verified by direct
    // reading, not assumed. If that's genuinely sufficient, this test proves it
    // empirically instead of a rewrite being carried out on the strength of the
    // ADR's own prose alone.

    #[tokio::test]
    async fn concurrent_slow_tenant_a_query_does_not_measurably_degrade_b_or_c_reads() {
        use std::time::Duration;

        // A real store with real, varied content (principle #14: no synthetic
        // placeholder used to dodge realistic load) large enough that a real
        // Cozo `search` scan takes measurable, non-trivial time -- empirically
        // tuned (500 nodes x 5 sequential searches ~= 150-800ms depending on
        // machine speed) to model the ADR's "tenant A runs a slow bulk query"
        // scenario without a fake sleep standing in for real work.
        let slow_store = mae_kb::CozoKbStore::open_mem().unwrap();
        for i in 0..500 {
            let body = format!(
                "Real body content for node {i}, discussing widgets, gadgets, and \
                 various engineering considerations relevant to search performance."
            );
            slow_store
                .insert_node(&mae_kb::Node::new(
                    format!("node:{i}"),
                    format!("Node {i} about widgets"),
                    mae_kb::NodeKind::Note,
                    &body,
                ))
                .unwrap();
        }

        let fast_store_b = mae_kb::CozoKbStore::open_mem().unwrap();
        fast_store_b
            .insert_node(&mae_kb::Node::new(
                "b-node",
                "Team B's note",
                mae_kb::NodeKind::Note,
                "team B content",
            ))
            .unwrap();
        let fast_store_c = mae_kb::CozoKbStore::open_mem().unwrap();
        fast_store_c
            .insert_node(&mae_kb::Node::new(
                "c-node",
                "Team C's note",
                mae_kb::NodeKind::Note,
                "team C content",
            ))
            .unwrap();

        let uuid_a = "uuid-slow-tenant".to_string();
        let uuid_b = "uuid-fast-tenant-b".to_string();
        let uuid_c = "uuid-fast-tenant-c".to_string();

        let mut st = DaemonState::new();
        st.store = Some(Arc::new(slow_store));
        st.instance_stores
            .insert(uuid_b.clone(), Arc::new(fast_store_b));
        st.instance_stores
            .insert(uuid_c.clone(), Arc::new(fast_store_c));
        let mk = |uuid: &str, name: &str, primary: bool| mae_kb::federation::KbInstance {
            uuid: uuid.to_string(),
            name: name.to_string(),
            org_dir: std::path::PathBuf::new(),
            db_path: std::path::PathBuf::new(),
            primary,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
            project_root: None,
            kind: mae_kb::federation::KbInstanceKind::default(),
            ingest_policy: Default::default(),
            priority: 0,
            remote_hub: None,
        };
        st.registry.instances.push(mk(&uuid_a, "team-a", true));
        st.registry.instances.push(mk(&uuid_b, "team-b", false));
        st.registry.instances.push(mk(&uuid_c, "team-c", false));
        st.rebuild_query_layer();
        let state = Arc::new(Mutex::new(st));

        // Solo baseline: B's read latency with no concurrent load at all --
        // the yardstick "measurably degrade" is measured against, per the
        // ADR's own verification language, rather than a hardcoded absolute
        // millisecond threshold (which would be flaky across CI runners of
        // different speeds).
        let baseline_start = Instant::now();
        dispatch(
            "kb/get",
            json!({"id": "b-node", "instance": uuid_b.clone()}),
            &state,
        )
        .await
        .unwrap();
        let solo_baseline = baseline_start.elapsed();

        // Now run tenant A's slow bulk query (5 sequential full-text scans over
        // 500 nodes) CONCURRENTLY with tenant B's and tenant C's single-node
        // reads, all racing on the one shared `Arc<Mutex<DaemonState>>`.
        let state_a = Arc::clone(&state);
        let slow_a = tokio::spawn(async move {
            let start = Instant::now();
            for _ in 0..5 {
                dispatch(
                    "kb/search",
                    json!({"query": "widgets", "limit": 1000, "instance": uuid_a.clone()}),
                    &state_a,
                )
                .await
                .unwrap();
            }
            start.elapsed()
        });

        // Give A's task a head start actually acquiring the lock and beginning
        // its blocking work, so B/C's reads land while A is genuinely in
        // flight, not merely scheduled.
        tokio::time::sleep(Duration::from_millis(5)).await;

        let state_b = Arc::clone(&state);
        let concurrent_b = tokio::spawn(async move {
            let start = Instant::now();
            dispatch(
                "kb/get",
                json!({"id": "b-node", "instance": uuid_b}),
                &state_b,
            )
            .await
            .unwrap();
            start.elapsed()
        });
        let state_c = Arc::clone(&state);
        let concurrent_c = tokio::spawn(async move {
            let start = Instant::now();
            dispatch(
                "kb/get",
                json!({"id": "c-node", "instance": uuid_c}),
                &state_c,
            )
            .await
            .unwrap();
            start.elapsed()
        });

        let a_duration = slow_a.await.unwrap();
        let b_duration = concurrent_b.await.unwrap();
        let c_duration = concurrent_c.await.unwrap();

        // Sanity: A's workload must actually be slow enough for this test to
        // mean anything -- if the machine is fast enough that A's 5-scan
        // workload finishes in a handful of milliseconds, the "did B/C wait
        // behind A" question isn't meaningfully exercised. 10ms is far below
        // what 5x full-text scans over 500 real nodes take even on a fast
        // release build (empirically ~150ms in dev-profile CI).
        assert!(
            a_duration > Duration::from_millis(10),
            "tenant A's workload wasn't actually slow enough to test against: {a_duration:?}"
        );

        // The property under test: B and C's reads, issued WHILE A's slow
        // query is in flight, must land close to their solo baseline -- not
        // anywhere near A's multi-hundred-millisecond workload duration. A
        // superficial Phase A/B implementation with one still-shared lock
        // held across the actual query would show B/C's latency rising in
        // lockstep with A's -- this generous-but-meaningful bound (10x the
        // solo baseline, or 50ms floor for a baseline too small to divide
        // sanely) catches exactly that regression while tolerating normal
        // scheduler jitter on a loaded CI runner.
        let tolerance = std::cmp::max(solo_baseline * 10, Duration::from_millis(50));
        assert!(
            b_duration < tolerance,
            "tenant B's read took {b_duration:?} while tenant A's slow query ({a_duration:?}) \
             was in flight -- solo baseline was {solo_baseline:?}, tolerance {tolerance:?}. \
             This is the specific regression Phase B's lock-splitting exists to prevent."
        );
        assert!(
            c_duration < tolerance,
            "tenant C's read took {c_duration:?} while tenant A's slow query ({a_duration:?}) \
             was in flight -- solo baseline was {solo_baseline:?}, tolerance {tolerance:?}."
        );
    }

    // --- ADR-060 Phase C: end-to-end enforcement through the real
    // `dispatch()` path, not just `TenantRegistry`'s own unit tests
    // (`tenant.rs`) -- proves the wiring itself (config -> DaemonState ->
    // `charge_tenant_or_reject` -> a real `DaemonError`), which the unit
    // tests alone can't catch a mistake in.

    fn tenant_config(
        name: &str,
        instances: &[&str],
        budget_per_minute: u32,
    ) -> crate::config::TenantConfig {
        crate::config::TenantConfig {
            name: name.to_string(),
            instances: instances.iter().map(|s| s.to_string()).collect(),
            principals: Vec::new(),
            quota: crate::config::TenantQuotaConfig {
                max_connections: 32,
                budget_per_minute,
                max_result_bytes: 4_194_304,
                idle_evict_secs: 1800,
            },
        }
    }

    #[tokio::test]
    async fn dispatch_rejects_a_quota_exceeding_tenant_with_a_real_daemon_error() {
        let (state, uuid_a, uuid_b, _uuid_c) = three_instance_state();
        {
            let mut st = state.lock().await;
            st.tenants = Arc::new(crate::tenant::TenantRegistry::from_config(&[
                tenant_config("team-a", &[&uuid_a], 1),
                tenant_config("team-b", &[&uuid_b], 1000),
            ]));
        }

        // team-a's budget (1 point) covers exactly one Read-cost kb/get.
        let r1 = dispatch(
            "kb/get",
            json!({"id": "shared-id", "instance": uuid_a}),
            &state,
        )
        .await;
        assert!(r1.is_ok(), "first request must be admitted: {r1:?}");

        // The second is over budget and must come back as a real,
        // distinguishable DaemonError -- not a generic failure, and not a
        // silent pass-through that ignores the quota entirely.
        let r2 = dispatch(
            "kb/get",
            json!({"id": "shared-id", "instance": uuid_a}),
            &state,
        )
        .await;
        match r2 {
            Err(DaemonError::QuotaExceeded(name)) => assert_eq!(name, uuid_a),
            other => panic!("expected QuotaExceeded, got {other:?}"),
        }

        // team-b, an entirely different tenant with an untouched budget, is
        // unaffected by team-a's exhaustion -- the real dispatch()/DaemonState
        // wiring preserves the isolation TenantRegistry's own unit tests prove
        // in isolation.
        let r3 = dispatch(
            "kb/get",
            json!({"id": "shared-id", "instance": uuid_b}),
            &state,
        )
        .await;
        assert!(r3.is_ok(), "team-b must be unaffected: {r3:?}");
    }

    #[tokio::test]
    async fn dispatch_cost_weights_scan_higher_than_read_through_the_real_path() {
        let (state, uuid_a, _uuid_b, _uuid_c) = three_instance_state();
        {
            let mut st = state.lock().await;
            // Budget covers exactly one Scan (3pts) or three Reads (1pt each).
            st.tenants = Arc::new(crate::tenant::TenantRegistry::from_config(&[
                tenant_config("team-a", &[&uuid_a], 3),
            ]));
        }

        let scan = dispatch(
            "kb/search",
            json!({"query": "note", "instance": uuid_a}),
            &state,
        )
        .await;
        assert!(scan.is_ok(), "one Scan must fit a 3-point budget: {scan:?}");

        let next = dispatch(
            "kb/get",
            json!({"id": "shared-id", "instance": uuid_a}),
            &state,
        )
        .await;
        assert!(
            matches!(next, Err(DaemonError::QuotaExceeded(_))),
            "budget must already be exhausted by the single 3-point Scan: {next:?}"
        );
    }

    #[tokio::test]
    async fn dispatch_admits_unconfigured_instances_unchanged_zero_tenant_tables() {
        // No `[[tenant]]` entries at all -- DaemonState::new()'s default
        // TenantRegistry::empty() must never reject anything, matching
        // Phase A's own zero-config-zero-behavior-change contract.
        let (state, uuid_a, _uuid_b, _uuid_c) = three_instance_state();
        for _ in 0..20 {
            let r = dispatch(
                "kb/get",
                json!({"id": "shared-id", "instance": uuid_a}),
                &state,
            )
            .await;
            assert!(
                r.is_ok(),
                "unconfigured tenants must never be rejected: {r:?}"
            );
        }
    }
}
