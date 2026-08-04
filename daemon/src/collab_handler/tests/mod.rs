// Test modules split from monolithic collab_handler_tests.rs (4,885 lines, 81 tests).

pub(crate) use super::*;
pub(crate) use crate::storage::SqliteBackend;
pub(crate) use mae_mcp::broadcast::EventBroadcaster;
pub(crate) use mae_sync::text::TextSync;

mod collab_handler_artifact_sharing_tests;
mod collab_handler_block_enforcement_tests;
mod collab_handler_connection_limits_tests;
mod collab_handler_cross_kb_node_isolation_tests;
mod collab_handler_cross_kb_role_isolation_tests;
mod collab_handler_derive_cache_tests;
mod collab_handler_governance_quorum_tests;
mod collab_handler_kb_lifecycle_tests;
mod collab_handler_kb_query_mtls_tests;
mod collab_handler_lease_race_tests;
mod collab_handler_legacy_migration_tests;
mod collab_handler_member_access_tests;
mod collab_handler_membership_join_tests;
mod collab_handler_n_way_convergence_tests;
mod collab_handler_persist_failure_tests;
mod collab_handler_protocol_dispatch_tests;
mod collab_handler_rebind_gate_tests;
mod collab_handler_recovery_key_tests;
mod collab_handler_replication_policy_tests;
mod collab_handler_self_issue_token_tests;
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
        &crate::artifact_store::NoArtifactStore,
        crate::kb_query::KbQueryLimits::default(),
        None,
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
        &crate::artifact_store::NoArtifactStore,
        crate::kb_query::KbQueryLimits::default(),
        None,
    )
    .await
}

/// Like [`dispatch_as`], but with an explicit `ArtifactStore` — for tests
/// exercising `kb/fetch_artifact` (ADR-061 Phase D3) against a fake with
/// pre-seeded cached vectors.
pub(crate) async fn dispatch_as_with_artifacts(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
    msg: serde_json::Value,
    docs: &mut HashSet<String>,
    artifact_store: &dyn crate::artifact_store::ArtifactStore,
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
        artifact_store,
        crate::kb_query::KbQueryLimits::default(),
        None,
    )
    .await
}

/// Like [`dispatch_as`], but with an explicit `self_issue` config — for
/// tests exercising `kb/query.self_token` (ADR-067 Phase D3), which needs a
/// real `Some(SelfIssueConfig)` rather than `dispatch_as`'s hardcoded
/// `None`.
pub(crate) async fn dispatch_as_with_self_issue(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
    msg: serde_json::Value,
    docs: &mut HashSet<String>,
    self_issue: Option<crate::oauth_self_issue::SelfIssueConfig>,
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
        &crate::artifact_store::NoArtifactStore,
        crate::kb_query::KbQueryLimits::default(),
        self_issue,
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

/// No RPC surface exists to set `ReplicationPolicy` on a member (that's
/// deliberately out of ADR-067 Phase B's scope -- Phase B is the gate, not
/// the admin command surface). Builds and signs a `SetRole(QueryOnly)` op
/// directly against the collection's own signed op-log, mirroring exactly
/// what `append_signed_membership` does internally for the RPC-driven path,
/// just with the one new field the RPC layer doesn't expose yet. Shared
/// between `collab_handler_replication_policy_tests.rs` (Phase B) and
/// `collab_handler_kb_query_mtls_tests.rs` (Phase D2) -- both need a real
/// QueryOnly member fixture.
pub(crate) async fn set_replication_query_only(
    store: &Arc<DocStore>,
    kb_id: &str,
    owner_secret: &[u8; 32],
    owner_pubkey: &[u8; 32],
    owner_fp: &str,
    subject: &str,
) {
    use mae_sync::membership::ReplicationPolicy;
    let mut coll = load_coll(store, kb_id).await;
    let epoch = coll.epoch_of(subject);
    let mut op = coll.build_membership_op(
        kb_id,
        MembershipAction::SetRole,
        subject,
        Some(SyncRole::Viewer),
        false,
        owner_fp,
        now_unix(),
        None,
        epoch,
    );
    op.replication = ReplicationPolicy::QueryOnly;
    let sig = op.sign(owner_secret);
    let update = coll.append_signed_op(&op, &sig, owner_pubkey);
    store
        .apply_update(&format!("kbc:{kb_id}"), &update, None)
        .await
        .unwrap();
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

pub(crate) fn kb_claim_lease_msg(kb_id: &str, op_kind: &str, ttl_secs: u64) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/claim_lease",
            "params":{"kb_id":kb_id,"op_kind":op_kind,"ttl_secs":ttl_secs}})
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
