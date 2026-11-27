//! ADR-113 PR-4 through the methods a client calls: a real `DocStore` on
//! SQLite, a KB owned by a freshly generated daemon key, members with real
//! fingerprints, and principals as the OAuth listener hands them over.

use std::sync::Arc;

use mae_mcp::identity::Identity;
use mae_mcp::protocol::McpError;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, Role, TransportPolicy};
use serde_json::{json, Value};

use crate::doc_editors::Attribution;
use crate::doc_store::DocStore;
use crate::kb_query::{self, KbQueryLimits};
use crate::oauth_self_issue::{Credential, PrincipalKind, ValidatedPrincipal};
use crate::storage::SqliteBackend;

mod act_tests;
mod attach_tests;
mod chain_store_tests;
mod fixture_shape_tests;
mod freeze_race_tests;
mod gate_tests;
mod import_tests;
mod limits_tests;
mod method_tests;
mod published_view_tests;
mod seed_author_tests;
mod strength_act_tests;
mod switch_race_tests;

pub(super) const KB: &str = "kb-review";
pub(super) const NODES: [&str; 3] = ["n1", "n2", "n3"];

pub(super) struct Env {
    pub store: Arc<DocStore>,
    pub backend: Arc<SqliteBackend>,
    pub daemon: Arc<Identity>,
    pub owner: String,
    pub editor: String,
    pub viewer: String,
    pub stranger: String,
}

/// A fresh, real fingerprint.
pub(super) fn fp(label: &str) -> String {
    Identity::generate(label).fingerprint()
}

pub(super) fn limits() -> KbQueryLimits {
    KbQueryLimits {
        max_body_bytes: 65_536,
        max_scan_nodes: 50,
        max_search_results: 20,
    }
}

/// A KB `kb` owned by `owner`, with an editor and a viewer, exposed over the
/// hub, holding `NODES`.
pub(super) async fn add_kb(store: &DocStore, kb: &str, owner: &str, editor: &str, viewer: &str) {
    let mut coll = KbCollectionDoc::new_owned(kb, owner, "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    coll.upsert_member(editor, "editor", Role::Editor);
    coll.upsert_member(viewer, "viewer", Role::Viewer);
    for n in NODES {
        let _ = coll.add_node(n, n);
    }
    let by = Attribution::Principal("seed");
    store
        .share_doc(&format!("kbc:{kb}"), &coll.encode_state(), by)
        .await
        .unwrap();
    for n in NODES {
        let node = KbNodeDoc::new(n, &format!("Title {n}"), &format!("Body of {n}."), &[]);
        store
            .share_doc(&mae_sync::kb_node_doc_name(kb, n), &node.encode_state(), by)
            .await
            .unwrap();
    }
}

pub(super) async fn env() -> Env {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    let store = Arc::new(DocStore::new(backend.clone(), 500));
    let daemon = Arc::new(Identity::generate("daemon"));
    store.set_signer(Arc::clone(&daemon));
    let (owner, editor, viewer) = (fp("owner"), fp("editor"), fp("viewer"));
    add_kb(&store, KB, &owner, &editor, &viewer).await;
    Env {
        store,
        backend,
        daemon,
        owner,
        editor,
        viewer,
        stranger: fp("stranger"),
    }
}

/// A principal as the listener hands it over: a bound IdP person.
pub(super) fn person(p: &str) -> ValidatedPrincipal {
    ValidatedPrincipal {
        principal: p.to_string(),
        audience: vec!["https://mae.example/mcp".into()],
        expires_at: u64::MAX,
        issuer: "https://idp.example".into(),
        subject: format!("sub-{}", &p[7..15]),
        credential: Credential::IdpBound,
        kind: PrincipalKind::Person,
        sign_in: None,
    }
}

/// Call `method` as `who`: `kb/review.*` through the review dispatcher, any
/// other through `kb/query`'s, as the OAuth listener routes them.
pub(super) async fn call(
    env: &Env,
    who: &str,
    method: &str,
    params: Value,
) -> Result<Value, McpError> {
    call_on(&env.store, who, method, params).await
}

pub(super) async fn call_on(
    store: &DocStore,
    who: &str,
    method: &str,
    params: Value,
) -> Result<Value, McpError> {
    if crate::review::is_review_method(method) {
        crate::review::dispatch(method, &params, store, &person(who), limits()).await
    } else {
        kb_query::dispatch(method, &params, store, Some(who), limits()).await
    }
}

pub(super) fn kind(e: &McpError) -> String {
    e.data.as_deref().expect("a kinded error")["kind"]
        .as_str()
        .unwrap()
        .to_string()
}

pub(super) fn err_kind(r: Result<Value, McpError>) -> String {
    match r {
        Ok(v) => panic!("expected a refusal, got {v}"),
        Err(e) => kind(&e),
    }
}

pub(super) async fn enable(env: &Env) {
    call(
        env,
        &env.owner,
        "kb/review.set_policy",
        json!({"kb_id": KB, "review_enabled": true, "code_prefixes": ["PRO"]}),
    )
    .await
    .unwrap();
}

/// The working copy's version of `node`.
pub(super) async fn version(env: &Env, node: &str) -> String {
    let state = env
        .store
        .encode_state(&mae_sync::kb_node_doc_name(KB, node))
        .await
        .unwrap();
    KbNodeDoc::from_bytes(&state).unwrap().version()
}

/// `who` edits `node` with `f`, as a client's update would.
pub(super) async fn edit_with(
    store: &DocStore,
    kb: &str,
    node: &str,
    who: &str,
    f: impl FnOnce(&mut KbNodeDoc) -> Vec<u8>,
) -> u64 {
    let name = mae_sync::kb_node_doc_name(kb, node);
    let state = store.encode_state(&name).await.unwrap();
    let mut doc = KbNodeDoc::from_bytes_with_client_id(&state, 777).unwrap();
    let update = f(&mut doc);
    store
        .apply_update(&name, &update, None, Attribution::Principal(who))
        .await
        .unwrap()
        .wal_seq
}

pub(super) async fn retitle(env: &Env, node: &str, title: &str) -> u64 {
    let editor = env.editor.clone();
    edit_with(&env.store, KB, node, &editor, |d| d.set_title(title)).await
}

/// A request for `nodes` at their current versions, by `who`.
pub(super) async fn request(env: &Env, who: &str, nodes: &[&str]) -> Result<Value, McpError> {
    let mut list = Vec::new();
    for n in nodes {
        list.push(json!({"node_id": n, "version": version(env, n).await}));
    }
    call(
        env,
        who,
        "kb/review.request",
        json!({"kb_id": KB, "nodes": list, "reason": "Annual revision."}),
    )
    .await
}

/// The KB's exported chain, verified under the daemon's key.
pub(super) async fn verified_events(env: &Env) -> u64 {
    let lines = crate::review::export_lines(&env.store, KB).await.unwrap();
    let text: String = lines.iter().map(|l| l.to_string() + "\n").collect();
    crate::review::chain::verify_export(&text, &env.daemon.public().to_bytes(), None)
        .unwrap()
        .events
}

/// The decoded event objects of the KB's chain, in order.
pub(super) async fn event_objects(env: &Env) -> Vec<Value> {
    crate::review::export_lines(&env.store, KB)
        .await
        .unwrap()
        .iter()
        .filter(|l| l["type"] == "event")
        .map(|l| {
            let b = mae_sync::encoding::base64_to_update(l["canonical"].as_str().unwrap()).unwrap();
            serde_json::from_slice(&b).unwrap()
        })
        .collect()
}
