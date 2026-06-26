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
                    "kind": node.kind.as_str(),
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
            let limit = std::cmp::min(params["limit"].as_u64().unwrap_or(20), 1000) as usize;
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

        "kb/neighborhood" => {
            let id = params["id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?;
            let depth = params["depth"].as_u64().unwrap_or(1) as u32;
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            match ql.neighborhood(id, depth) {
                Some(sg) => Ok(json!({
                    "nodes": sg.nodes.iter().map(|(id, t)| json!([id, t])).collect::<Vec<_>>(),
                    "edges": sg.edges.iter().map(|(s, d, r)| json!([s, d, r])).collect::<Vec<_>>(),
                })),
                None => Ok(json!({"nodes": [], "edges": []})),
            }
        }

        "kb/related" => {
            let id = params["id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'id'"))?;
            let limit = std::cmp::min(params["limit"].as_u64().unwrap_or(10), 1000) as usize;
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            let related: Vec<Value> = ql
                .related(id, limit)
                .into_iter()
                .map(|(id, score)| json!([id, score]))
                .collect();
            Ok(json!(related))
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
            let doc_store = { state.lock().await.doc_store.clone() };
            let ds = doc_store.ok_or(DaemonError::NotReady)?;
            let doc_name = format!("kb:{id}");
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
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            let nodes: Vec<Value> = ql
                .todo_nodes()
                .into_iter()
                .map(|mut n| {
                    n.crdt_doc = None;
                    serde_json::to_value(&n).unwrap_or(Value::Null)
                })
                .collect();
            Ok(json!(nodes))
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
            let body_limit =
                std::cmp::min(params["body_limit"].as_u64().unwrap_or(0), 10_000) as usize;
            let state = state.lock().await;
            let ql = state.query_layer.as_ref().ok_or(DaemonError::NotReady)?;
            let triples: Vec<Value> = ql
                .id_title_body_triples(prefix, body_limit)
                .into_iter()
                .map(|(id, title, body)| json!([id, title, body]))
                .collect();
            Ok(json!(triples))
        }

        // --- Hygiene ---
        "kb/hygiene_scan" => {
            let state = state.lock().await;
            let store = state.store.as_ref().ok_or(DaemonError::NotReady)?;
            let result = crate::hygiene::run_hygiene_scan(store);
            Ok(json!({
                "suggestions_created": result.suggestions_created,
                "nodes_scanned": result.nodes_scanned,
                "errors": result.errors,
            }))
        }

        "kb/hygiene_report" => {
            let category = params["category"].as_str();
            let status = params["status"].as_str();
            let state = state.lock().await;
            let store = state.store.as_ref().ok_or(DaemonError::NotReady)?;
            let suggestions = store
                .list_suggestions(category, status)
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
        }

        "kb/hygiene_accept" => {
            let node_id = params["node_id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'node_id'"))?;
            let suggestion_id = params["suggestion_id"]
                .as_i64()
                .ok_or(DaemonError::InvalidParams("missing 'suggestion_id'"))?;
            let state = state.lock().await;
            let store = state.store.as_ref().ok_or(DaemonError::NotReady)?;
            store
                .update_suggestion_status(node_id, suggestion_id, "accepted")
                .map_err(|e| DaemonError::Internal(e.to_string()))?;
            Ok(json!({"ok": true}))
        }

        "kb/hygiene_dismiss" => {
            let node_id = params["node_id"]
                .as_str()
                .ok_or(DaemonError::InvalidParams("missing 'node_id'"))?;
            let suggestion_id = params["suggestion_id"]
                .as_i64()
                .ok_or(DaemonError::InvalidParams("missing 'suggestion_id'"))?;
            let state = state.lock().await;
            let store = state.store.as_ref().ok_or(DaemonError::NotReady)?;
            store
                .update_suggestion_status(node_id, suggestion_id, "dismissed")
                .map_err(|e| DaemonError::Internal(e.to_string()))?;
            Ok(json!({"ok": true}))
        }

        // --- Lifecycle ---
        "daemon/status" => {
            // Snapshot the fields, then drop the lock before the async doc_store scan
            // (don't hold the state mutex across an await).
            let (uptime, store_count, has_ql, reg_count, doc_store) = {
                let state = state.lock().await;
                (
                    state.started_at.elapsed(),
                    1 + state.instance_stores.len(),
                    state.query_layer.is_some(),
                    state.registry.instances.len(),
                    state.doc_store.clone(),
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
            }))
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
                let doc_store = st.doc_store.clone().ok_or_else(|| {
                    DaemonError::Internal(
                        "P2P sharing is unavailable — enable collab key mode + the mesh \
                         (`mae setup-collab --p2p`) and restart the daemon"
                            .to_string(),
                    )
                })?;
                let broadcaster = st.broadcaster.clone().ok_or_else(|| {
                    DaemonError::Internal("collab broadcaster unavailable".into())
                })?;
                let owner = st.owner.clone().ok_or_else(|| {
                    DaemonError::Internal("daemon owner identity unavailable".into())
                })?;
                // The CozoDB store backing this KB (primary or a named instance), so
                // a fresh share can SEED node content — not just the collection.
                let kb_store = resolve_kb_store(&st, &kb_id);
                (doc_store, broadcaster, owner, kb_store)
            };
            // Build the collection + node states from the daemon's KB store (outside
            // the state lock — `load_all` is a blocking CozoDB read). Reuses the same
            // `KnowledgeBase::to_collection` the editor's `kb/share` uses, so the
            // seeded node docs are byte-identical. Absent store / empty KB ⇒ an empty
            // collection (still a valid mesh share at collection level).
            let seed = match kb_store {
                Some(store) => {
                    let nodes = store
                        .load_all()
                        .map_err(|e| DaemonError::Internal(format!("load KB nodes: {e}")))?;
                    let mut kb = mae_kb::KnowledgeBase::new();
                    for node in nodes {
                        kb.insert(node);
                    }
                    Some(
                        kb.to_collection(&kb_id, &owner.fingerprint(), &[])
                            .map_err(|e| {
                                DaemonError::Internal(format!(
                                    "build collection from KB store: {e}"
                                ))
                            })?,
                    )
                }
                None => None,
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

/// A store-seeded collection ready to share: the collection doc (manifest +
/// owner/policy) plus each node's `(node_id, encoded yrs state)`.
type SeededCollection = (mae_sync::kb::KbCollectionDoc, Vec<(String, Vec<u8>)>);

/// Resolve the CozoDB store backing `kb_id` (a KB *name*): the primary KB's own
/// store, or a named instance's store. `None` when the name isn't registered with
/// this daemon — the share still proceeds at collection level, just without seeded
/// node content.
fn resolve_kb_store(st: &DaemonState, kb_id: &str) -> Option<Arc<CozoKbStore>> {
    let inst = st.registry.find(kb_id)?;
    if inst.primary {
        st.store.clone()
    } else {
        st.instance_stores.get(&inst.uuid).cloned()
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
        doc_store
            .share_doc(&collection_doc, &coll.encode_state())
            .await
            .map_err(|e| DaemonError::Internal(format!("share collection: {e}")))?;
        // Seed each node doc (`kb:{node_id}`) so a joining peer pulls real content,
        // not just the manifest. Same naming + encoding as the TCP `kb/share` path.
        for (node_id, state) in &node_states {
            let node_doc = format!("kb:{node_id}");
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
        ds.share_doc("kb:concept:x", &node.encode()).await.unwrap();
        let mut st = DaemonState::new();
        st.doc_store = Some(ds);
        let state = Arc::new(Mutex::new(st));

        // Hosted node → base64 CRDT state.
        let r = dispatch("kb/node_crdt", json!({"id": "concept:x"}), &state)
            .await
            .unwrap();
        assert!(
            r["state"].as_str().is_some(),
            "hosted node must return CRDT state: {r}"
        );
        // Absent node → null (no spurious empty-doc materialization).
        let r2 = dispatch("kb/node_crdt", json!({"id": "concept:absent"}), &state)
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
        let result = dispatch("p2p/mint_ticket", json!({"kb_id": "kb:x"}), &state).await;
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
        // A real minted ticket round-trips through the join method.
        let id = mae_mcp::identity::Identity::generate("owner");
        let endpoint = crate::p2p::bind_endpoint(&id, iroh::RelayMode::Disabled)
            .await
            .unwrap();
        let ticket = crate::p2p::mint_ticket(&endpoint, "concept:x").to_string();
        endpoint.close().await;

        let state = Arc::new(Mutex::new(DaemonState::new()));
        let result = dispatch("p2p/join_ticket", json!({ "ticket": ticket }), &state)
            .await
            .unwrap();
        assert_eq!(result["kb_id"].as_str(), Some("concept:x"));
        assert_eq!(result["status"].as_str(), Some("recorded"));
        assert!(result["peer"].as_str().unwrap().starts_with("SHA256:"));
        assert_eq!(state.lock().await.pending_p2p_joins.len(), 1);

        // Re-accepting the same ticket does not double-queue.
        dispatch("p2p/join_ticket", json!({ "ticket": ticket }), &state)
            .await
            .unwrap();
        assert_eq!(state.lock().await.pending_p2p_joins.len(), 1);
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
            .encode_state_and_sv("kb:collabtest:overview")
            .await
            .unwrap();
        let node_doc = KbNodeDoc::from_bytes(&nbytes).unwrap();
        assert!(
            node_doc.body().contains("ZEPHYRINE"),
            "seeded node doc must carry the body content"
        );
    }
}
