//! Every refusal on the collab listener carries `error.data.kind`, so a client
//! branches on a stable string instead of parsing English (and instead of
//! re-reading the collection to guess what went wrong).
//!
//! Table-driven over the refusal inventory, through the REAL dispatch
//! (`handle_doc_request_inner` via `dispatch_as`), against one real KB with an
//! owner, an editor, a viewer and a stranger. Each row names who asks, what
//! they send, the kind they must get and any extra `data` field that kind
//! carries. The write paths (epoch fence, signatures, storage faults) and the
//! non-member no-oracle property have their own files.

use super::*;

pub(super) const KB: &str = "ek";
pub(super) const NODE: &str = "concept:n";

pub(super) struct Fixture {
    pub store: Arc<DocStore>,
    pub bc: SharedBroadcaster,
    pub docs: HashSet<String>,
}

impl Fixture {
    /// Dispatch as `who` (label; principal `fp(who)`), or unauthenticated.
    pub async fn call(&mut self, who: Option<&str>, msg: serde_json::Value) -> JsonRpcResponse {
        let principal = who.map(fp);
        self.call_as(principal.as_deref(), msg).await
    }

    /// Dispatch as an explicit principal (a real key fingerprint).
    pub async fn call_as(
        &mut self,
        principal: Option<&str>,
        msg: serde_json::Value,
    ) -> JsonRpcResponse {
        dispatch_as(
            &self.store,
            &self.bc,
            principal,
            principal,
            msg,
            &mut self.docs,
        )
        .await
    }
}

pub(super) fn rpc(method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
}

/// A valid one-op CRDT update, authored under `(principal, epoch)`'s client id.
pub(super) fn update_as(principal: &str, epoch: u64, text: &str) -> String {
    let mut ts = TextSync::with_client_id("", derive_kb_client_id(principal, epoch));
    update_to_base64(&ts.insert(0, text))
}

/// The error's `data` object, asserting it has the expected kind.
pub(super) fn expect_kind(resp: &JsonRpcResponse, kind: &str, ctx: &str) -> serde_json::Value {
    let err = resp
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("{ctx}: expected a `{kind}` refusal, got {:?}", resp.result));
    let data = err.data.as_deref().cloned().unwrap_or_default();
    assert_eq!(data["kind"], kind, "{ctx}: {} — data {data}", err.message);
    assert!(
        !err.message.is_empty(),
        "{ctx}: the message is kept for logs"
    );
    data
}

/// A KB with an owner, an editor and a viewer, one node with content, and a
/// restrictive join policy (so a stranger's join is refused, not pending).
pub(super) async fn fixture() -> Fixture {
    let mut f = Fixture {
        store: test_doc_store(),
        bc: test_broadcaster(),
        docs: HashSet::new(),
    };
    let owner = fp("owner");
    let shared = kb_share_as(
        &f.store,
        &f.bc,
        Some("owner"),
        Some(&owner),
        KB,
        "owner",
        &mut f.docs,
    )
    .await;
    assert!(shared.error.is_none(), "share: {:?}", shared.error);
    for (who, role) in [("editor", "editor"), ("viewer", "viewer")] {
        let r = f
            .call(
                Some("owner"),
                kb_member_msg("kb/add_member", KB, &fp(who), Some(role)),
            )
            .await;
        assert!(r.error.is_none(), "add {who}: {:?}", r.error);
    }
    let add = rpc(
        "kb/collection_node_add",
        serde_json::json!({"kb_id": KB, "node_id": NODE, "title": "n"}),
    );
    assert!(f.call(Some("owner"), add).await.error.is_none());
    let epoch = load_coll(&f.store, KB).await.epoch_of(&owner);
    let write = rpc(
        "kb/node_update",
        serde_json::json!({"kb_id": KB, "node_id": NODE, "update": update_as(&owner, epoch, "body")}),
    );
    let w = f.call(Some("owner"), write).await;
    assert!(w.error.is_none(), "owner writes the node: {:?}", w.error);
    assert!(f
        .call(Some("owner"), kb_policy_msg(KB, "restrictive"))
        .await
        .error
        .is_none());
    f
}

/// One row: who asks, what, the kind, and an extra `data` field (name, value).
struct Case {
    who: Option<&'static str>,
    msg: serde_json::Value,
    kind: &'static str,
    extra: Option<(&'static str, serde_json::Value)>,
}

fn case(who: &'static str, msg: serde_json::Value, kind: &'static str) -> Case {
    Case {
        who: Some(who),
        msg,
        kind,
        extra: None,
    }
}

impl Case {
    fn with(mut self, field: &'static str, value: &str) -> Self {
        self.extra = Some((field, serde_json::json!(value)));
        self
    }
}

fn node_fetch(kb: &str, node: &str) -> serde_json::Value {
    rpc(
        "kb/node_fetch",
        serde_json::json!({"kb_id": kb, "node_id": node}),
    )
}

fn query_get(kb: &str, node: &str) -> serde_json::Value {
    rpc(
        "kb/query.get",
        serde_json::json!({"kb_id": kb, "node_id": node}),
    )
}

fn node_update(update: String, extra: serde_json::Value) -> serde_json::Value {
    let mut params = serde_json::json!({"kb_id": KB, "node_id": NODE, "update": update});
    for (k, v) in extra.as_object().into_iter().flatten() {
        params[k] = v.clone();
    }
    rpc("kb/node_update", params)
}

/// Membership and existence refusals from the access engine.
fn membership_cases() -> Vec<Case> {
    use serde_json::json;
    let kbc = format!("kbc:{KB}");
    let awareness = rpc("sync/awareness", json!({"doc": kbc, "state": {}}));
    vec![
        case("stranger", node_fetch(KB, NODE), "not_member"),
        case("stranger", kb_join_msg(KB), "not_member"),
        case("stranger", query_get(KB, NODE), "not_member"),
        case(
            "stranger",
            rpc("sync/state_vector", json!({"doc": kbc})),
            "not_member",
        ),
        case("stranger", awareness, "not_member"),
        case("owner", node_fetch("no-such-kb", NODE), "kb_not_found"),
        case("owner", query_get("no-such-kb", NODE), "kb_not_found"),
        case("viewer", node_fetch(KB, "concept:absent"), "node_not_in_kb"),
        case("viewer", query_get(KB, "concept:absent"), "node_not_in_kb"),
    ]
}

/// Role refusals: a member whose role is below what the operation needs.
fn role_cases() -> Vec<Case> {
    let viewer_write = node_update(update_as(&fp("viewer"), 0, "v"), serde_json::json!({}));
    let smuggle = rpc(
        "sync/update",
        serde_json::json!({"doc": format!("kbc:{KB}"), "update": update_as("x", 0, "x")}),
    );
    let add = kb_member_msg("kb/add_member", KB, &fp("x"), Some("viewer"));
    vec![
        case("viewer", viewer_write, "role_forbidden").with("required", "editor"),
        case(
            "viewer",
            kb_claim_lease_msg(KB, "sweep", 60),
            "role_forbidden",
        )
        .with("required", "editor"),
        case("editor", add, "role_forbidden").with("required", "owner"),
        case("editor", kb_policy_msg(KB, "invite"), "role_forbidden").with("required", "owner"),
        case(
            "editor",
            kb_approve_msg(KB, &fp("x"), None),
            "role_forbidden",
        )
        .with("required", "owner"),
        case("editor", smuggle, "role_forbidden").with("required", "owner"),
        case(
            "editor",
            kb_collection_op_msg(KB, b"\x00"),
            "role_forbidden",
        )
        .with("required", "owner"),
    ]
}

/// Malformed parameters, including an id no document address can carry.
fn param_cases() -> Vec<Case> {
    use serde_json::json;
    let node_doc = mae_sync::kb_node_doc_name(KB, NODE);
    let unscoped = rpc(
        "sync/update",
        json!({"doc": node_doc, "update": update_as("e", 0, "e")}),
    );
    vec![
        case("owner", rpc("kb/join", json!({})), "invalid_params").with("param", "kb_id"),
        case("owner", kb_policy_msg(KB, "bogus"), "invalid_params").with("param", "policy"),
        case("owner", node_fetch("a:b", NODE), "invalid_params").with("param", "kb_id"),
        case(
            "owner",
            node_update("@@".into(), json!({})),
            "invalid_params",
        )
        .with("param", "update"),
        case("editor", unscoped, "invalid_params").with("param", "kb_id"),
        case(
            "owner",
            rpc("sync/diff", json!({"doc": "plain", "sv": "@@"})),
            "invalid_params",
        )
        .with("param", "sv"),
    ]
}

/// Document-address and operation refusals.
fn operation_cases(owner_epoch: u64) -> Vec<Case> {
    use serde_json::json;
    let node_doc = mae_sync::kb_node_doc_name(KB, NODE);
    let rebirth = node_update(
        update_as(&fp("owner"), owner_epoch, "o"),
        json!({"rebirth": true}),
    );
    let self_token = || rpc("kb/query.self_token", json!({}));
    vec![
        case(
            "viewer",
            rpc("sync/full_state", json!({"doc": node_doc})),
            "doc_unavailable",
        ),
        case(
            "owner",
            rpc("docs/content", json!({"doc": format!("kbc:{KB}")})),
            "doc_unavailable",
        ),
        case(
            "owner",
            kb_member_msg("kb/remove_member", KB, &fp("owner"), None),
            "cannot_remove_owner",
        ),
        case(
            "owner",
            kb_set_governance_msg(KB, "single-owner"),
            "signer_unavailable",
        ),
        case(
            "owner",
            kb_revoke_msg(KB, &fp("editor")),
            "signer_unavailable",
        ),
        Case {
            who: None,
            msg: self_token(),
            kind: "not_authenticated",
            extra: None,
        },
        case("owner", self_token(), "self_issue_disabled"),
        case("owner", rebirth, "rebirth_unverified"),
    ]
}

#[tokio::test]
async fn every_refusal_in_the_inventory_carries_its_kind() {
    let mut f = fixture().await;
    let owner_epoch = load_coll(&f.store, KB).await.epoch_of(&fp("owner"));
    let cases = membership_cases()
        .into_iter()
        .chain(role_cases())
        .chain(param_cases())
        .chain(operation_cases(owner_epoch));
    for (i, c) in cases.enumerate() {
        let ctx = format!("case {i}: {:?} {}", c.who, c.msg);
        let resp = f.call(c.who, c.msg).await;
        let data = expect_kind(&resp, c.kind, &ctx);
        if let Some((field, value)) = c.extra {
            assert_eq!(data[field], value, "{ctx}: data.{field}");
        }
    }
}

/// The same refusal carries the same kind on whichever listener reaches it:
/// `kb/query.*` on the collab socket and through `kb_query::dispatch` directly
/// (what the HTTPS listener calls) agree, including for the methods only the
/// HTTPS listener routes.
#[tokio::test]
async fn kb_query_refusals_agree_across_entry_points() {
    let f = fixture().await;
    let limits = crate::kb_query::KbQueryLimits::default();
    let cases = [
        ("kb/query.links", fp("stranger"), NODE, "not_member"),
        ("kb/query.neighborhood", fp("stranger"), NODE, "not_member"),
        (
            "kb/query.links",
            fp("viewer"),
            "concept:absent",
            "node_not_in_kb",
        ),
        (
            "kb/query.neighborhood",
            fp("viewer"),
            "concept:absent",
            "node_not_in_kb",
        ),
        (
            "kb/query.get",
            fp("viewer"),
            "concept:absent",
            "node_not_in_kb",
        ),
    ];
    for (method, who, node, kind) in cases {
        let params = serde_json::json!({"kb_id": KB, "node_id": node});
        let err = crate::kb_query::dispatch(method, &params, &f.store, Some(&who), limits)
            .await
            .expect_err(method);
        let data = err.data.as_deref().cloned().unwrap_or_default();
        assert_eq!(
            data["kind"], kind,
            "{method} as {who} on {node}: {}",
            err.message
        );
    }
}
