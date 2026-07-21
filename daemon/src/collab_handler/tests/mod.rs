// Test modules split from monolithic collab_handler_tests.rs (4,885 lines, 81 tests).

pub(crate) use super::*;
pub(crate) use crate::storage::SqliteBackend;
pub(crate) use mae_mcp::broadcast::EventBroadcaster;
pub(crate) use mae_sync::text::TextSync;

mod collab_handler_block_enforcement_tests;
mod collab_handler_connection_limits_tests;
mod collab_handler_derive_cache_tests;
mod collab_handler_governance_quorum_tests;
mod collab_handler_kb_lifecycle_tests;
mod collab_handler_legacy_migration_tests;
mod collab_handler_member_access_tests;
mod collab_handler_membership_join_tests;
mod collab_handler_n_way_convergence_tests;
mod collab_handler_persist_failure_tests;
mod collab_handler_protocol_dispatch_tests;
mod collab_handler_rebind_gate_tests;
mod collab_handler_recovery_key_tests;
mod collab_handler_signed_content_relay_tests;
mod collab_handler_sync_protocol_tests;
mod collab_handler_transport_oplog_tests;
mod collab_handler_viewer_epoch_tests;

// Shared test helpers/fixtures used across multiple test modules

pub(crate) fn test_broadcaster() -> SharedBroadcaster {
    Arc::new(std::sync::Mutex::new(EventBroadcaster::new()))
}

pub(crate) fn test_doc_store() -> Arc<DocStore> {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    Arc::new(DocStore::new(backend, 500))
}

pub(crate) fn make_test_node(id: &str, title: &str, body: &str, tags: &[&str]) -> Vec<u8> {
    use mae_sync::kb::KbNodeDoc;
    let node = KbNodeDoc::new(
        id,
        title,
        body,
        &tags.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    );
    node.encode()
}

pub(crate) fn realistic_org_body() -> &'static str {
    ":PROPERTIES:\n:ID: test-node-001\n:ROAM_REFS: https://example.com\n:END:\n\
         #+TITLE: Test Node — CRDT Round-Trip\n#+FILETAGS: :research:crdt:\n\n\
         * Overview\n\
         This node tests the full round-trip: SQLite → KbNodeDoc → base64 → server → base64 → KbNodeDoc → SQLite.\n\n\
         ** Sub-heading with [[id:other-node][internal link]]\n\
         Content with Unicode: café, naïve, 日本語\n\n\
         #+begin_src rust\nfn main() { println!(\"hello\"); }\n#+end_src\n"
}

pub(crate) fn fp(label: &str) -> String {
    format!("SHA256:{label}")
}

pub(crate) async fn kb_share_as(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
    kb_id: &str,
    claimed_creator: &str,
    session_docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    let coll = KbCollectionDoc::new_owned(kb_id, "", auth_label.unwrap_or(""));
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": {
            "kb_id": kb_id,
            "name": kb_id,
            "creator": claimed_creator,
            "collection_state": update_to_base64(&coll.encode_state()),
            "nodes": [],
        }
    });
    handle_doc_request_inner(
        &msg.to_string(),
        store,
        bc,
        std::time::Instant::now(),
        0,
        auth_label,
        auth_principal,
        None,
        session_docs,
        Transport::Hub,
    )
    .await
}

pub(crate) async fn dispatch_as(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
    msg: serde_json::Value,
    docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    handle_doc_request_inner(
        &msg.to_string(),
        store,
        bc,
        std::time::Instant::now(),
        0,
        auth_label,
        auth_principal,
        None,
        docs,
        Transport::Hub,
    )
    .await
}

pub(crate) async fn load_coll(store: &Arc<DocStore>, kb_id: &str) -> KbCollectionDoc {
    let (state, _) = store
        .encode_state_and_sv(&format!("kbc:{kb_id}"))
        .await
        .expect("collection exists");
    KbCollectionDoc::from_bytes(&state).expect("valid collection")
}

pub(crate) fn kb_join_msg(kb_id: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/join","params":{"kb_id":kb_id}})
}

pub(crate) fn kb_node_update_msg(kb_id: &str) -> serde_json::Value {
    let mut ts = TextSync::with_client_id("", 7);
    let upd = ts.insert(0, "x");
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":kb_id,"node_id":"concept:n","update":update_to_base64(&upd)}})
}

pub(crate) fn kb_node_update_msg_as(
    kb_id: &str,
    principal: &str,
    epoch: u64,
    text: &str,
) -> serde_json::Value {
    let cid = derive_kb_client_id(principal, epoch);
    let mut ts = TextSync::with_client_id("", cid);
    let upd = ts.insert(0, text);
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":kb_id,"node_id":"concept:n","update":update_to_base64(&upd)}})
}

pub(crate) fn kb_member_msg(
    method: &str,
    kb_id: &str,
    member: &str,
    role: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,
            "params":{"kb_id":kb_id,"member":member,"role":role,"label":member}})
}

pub(crate) fn kb_policy_msg(kb_id: &str, policy: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/set_policy",
            "params":{"kb_id":kb_id,"policy":policy}})
}

pub(crate) fn kb_approve_msg(
    kb_id: &str,
    principal: &str,
    role: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/approve_member",
            "params":{"kb_id":kb_id,"principal":principal,"role":role}})
}

pub(crate) async fn share_kb_with_nodes(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    kb_id: &str,
    name: &str,
    creator: &str,
    nodes: &[(&str, Vec<u8>)],
    session_docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    use mae_sync::kb::KbCollectionDoc;

    let mut coll = KbCollectionDoc::new(name, creator);
    for (id, _) in nodes {
        coll.add_node(id, id); // title = id for simplicity
    }
    let collection_b64 = update_to_base64(&coll.encode_state());

    let nodes_json: Vec<serde_json::Value> = nodes
        .iter()
        .map(|(id, state)| serde_json::json!({ "id": id, "state": update_to_base64(state) }))
        .collect();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": {
            "kb_id": kb_id,
            "name": name,
            "creator": creator,
            "collection_state": collection_b64,
            "nodes": nodes_json,
        }
    });
    handle_doc_request(
        &msg.to_string(),
        store,
        bc,
        std::time::Instant::now(),
        0,
        session_docs,
    )
    .await
}

pub(crate) fn kb_block_msg(method: &str, kb_id: &str, principal: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,
            "params":{"kb_id":kb_id,"fingerprint":principal}})
}

pub(crate) fn kb_set_governance_msg(kb_id: &str, governance: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/set_governance",
            "params":{"kb_id":kb_id,"governance":governance}})
}

pub(crate) fn kb_revoke_msg(kb_id: &str, member: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/revoke",
            "params":{"kb_id":kb_id,"member":member}})
}

pub(crate) fn signed_node_update_msg(
    kb_id: &str,
    node_id: &str,
    update: &[u8],
    signed: &mae_sync::content_ops::SignedContentOp,
) -> serde_json::Value {
    let mut params = serde_json::json!({
        "kb_id": kb_id,
        "node_id": node_id,
        "update": update_to_base64(update),
    });
    for (k, v) in signed.header_params().as_object().unwrap() {
        params[k] = v.clone();
    }
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update","params":params})
}

pub(crate) fn rotor_keys(seed: u8) -> ([u8; 32], [u8; 32], String, [u8; 32]) {
    let id = mae_mcp::identity::Identity::from_seed(&[seed; 32], "k");
    let secret = id.secret_bytes();
    let pubkey = id.public().to_bytes();
    let fp = mae_sync::membership::fingerprint_of(&pubkey);
    let wrap = mae_sync::content_crypto::wrap_public_for(&secret);
    (secret, pubkey, fp, wrap)
}

pub(crate) fn kb_collection_op_msg(kb_id: &str, update: &[u8]) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/collection_op",
        "params":{"kb_id":kb_id,"update":update_to_base64(update)}})
}

pub(crate) async fn kb_with_member(
    kb_id: &str,
    member_seed: u8,
) -> (
    Arc<DocStore>,
    SharedBroadcaster,
    ([u8; 32], [u8; 32], String, [u8; 32]),
    HashSet<String>,
) {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let owner = fp("owner");
    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        kb_id,
        "owner",
        &mut docs,
    )
    .await;
    let m = rotor_keys(member_seed);
    let r = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        kb_member_msg("kb/add_member", kb_id, &m.2, Some("editor")),
        &mut docs,
    )
    .await;
    assert!(r.error.is_none(), "owner admits the member: {:?}", r.error);
    (store, bc, m, docs)
}
