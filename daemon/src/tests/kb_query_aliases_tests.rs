//! `kb/query.get` and `kb/query.list` report a node's aliases by the one rule
//! in `mae_sync::kb::aliases`: in an `org-literal-v1` KB derived from the
//! `ROAM_ALIASES` property, in a native KB the node's array.

use std::collections::HashMap;
use std::sync::Arc;

use mae_daemon::doc_store::DocStore;
use mae_daemon::kb_query::{self, KbQueryLimits};
use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, Role, BODY_FORMAT_ORG_LITERAL_V1};
use serde_json::{json, Value};

use super::kb_query_tests::{fresh_doc_store, generous_limits, seed_unencrypted_kb};

const TEST_BY: mae_daemon::doc_editors::Attribution<'static> =
    mae_daemon::doc_editors::Attribution::Principal("test-editor");
const VIEWER: &str = "oauth:viewer@example.com";

/// One KB `kb` with node `n1`, its body format set to `format`, its
/// `ROAM_ALIASES` to `roam` and its array to `array`.
async fn seed(kb: &str, format: Option<&str>, roam: &str, array: &[&str]) -> Arc<DocStore> {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    let viewer = Some((VIEWER, Role::Viewer));
    seed_unencrypted_kb(&doc_store, &owner, kb, viewer, "n1", "Plain", "body", &[]).await;
    if let Some(f) = format {
        let state = doc_store.encode_state(&format!("kbc:{kb}")).await.unwrap();
        let mut coll = KbCollectionDoc::from_bytes(&state).unwrap();
        let update = coll.set_body_format(f);
        apply(&doc_store, &format!("kbc:{kb}"), &update).await;
    }
    set_roam_aliases(&doc_store, kb, roam).await;
    let array: Vec<String> = array.iter().map(|s| s.to_string()).collect();
    edit(&doc_store, kb, |d| d.set_aliases(&array)).await;
    doc_store
}

async fn apply(doc_store: &DocStore, name: &str, update: &[u8]) {
    doc_store
        .apply_update(name, update, None, TEST_BY)
        .await
        .unwrap();
}

async fn edit(doc_store: &DocStore, kb: &str, f: impl FnOnce(&mut KbNodeDoc) -> Vec<u8>) {
    let name = mae_sync::kb_node_doc_name(kb, "n1");
    let state = doc_store.encode_state(&name).await.unwrap();
    let mut doc = KbNodeDoc::from_bytes_with_client_id(&state, 777).unwrap();
    let update = f(&mut doc);
    apply(doc_store, &name, &update).await;
}

async fn set_roam_aliases(doc_store: &DocStore, kb: &str, value: &str) {
    let props = HashMap::from([
        ("ID".to_string(), "n1".to_string()),
        ("ROAM_ALIASES".to_string(), value.to_string()),
    ]);
    edit(doc_store, kb, |d| d.set_properties(&props)).await;
}

async fn call(doc_store: &DocStore, method: &str, kb: &str) -> Value {
    let limits: KbQueryLimits = generous_limits();
    kb_query::dispatch(
        method,
        &json!({"kb_id": kb, "node_id": "n1"}),
        doc_store,
        Some(VIEWER),
        limits,
    )
    .await
    .unwrap_or_else(|e| panic!("{method}: {e:?}"))
}

/// `get`'s node and `list`'s single node, which must agree.
async fn both(doc_store: &DocStore, kb: &str) -> [Value; 2] {
    let got = call(doc_store, "kb/query.get", kb).await;
    let listed = call(doc_store, "kb/query.list", kb).await["nodes"][0].clone();
    [got, listed]
}

#[tokio::test]
async fn org_literal_get_and_list_report_the_roam_aliases() {
    let ds = seed(
        "lit",
        Some(BODY_FORMAT_ORG_LITERAL_V1),
        "foo \"bar baz\"",
        &[],
    )
    .await;
    for r in both(&ds, "lit").await {
        assert_eq!(r["aliases"], json!(["foo", "bar baz"]), "{r}");
        assert!(r.get("aliases_problems").is_none(), "{r}");
    }
}

#[tokio::test]
async fn editing_the_property_changes_the_reported_aliases() {
    let ds = seed("lit", Some(BODY_FORMAT_ORG_LITERAL_V1), "foo", &[]).await;
    set_roam_aliases(&ds, "lit", "\"two words\" \"a \\\"q\\\"\"").await;
    for r in both(&ds, "lit").await {
        assert_eq!(r["aliases"], json!(["two words", "a \"q\""]), "{r}");
    }
}

#[tokio::test]
async fn native_kb_reports_the_array() {
    let ds = seed("nat", None, "from-property", &["from-array", "second"]).await;
    for r in both(&ds, "nat").await {
        assert_eq!(r["aliases"], json!(["from-array", "second"]), "{r}");
        assert!(r.get("aliases_problems").is_none(), "{r}");
    }
}

/// The array is not a source in an org-literal KB: its values are not
/// reported, and the reader is told it is set.
#[tokio::test]
async fn org_literal_does_not_report_an_illegal_array() {
    let ds = seed(
        "lit",
        Some(BODY_FORMAT_ORG_LITERAL_V1),
        "real",
        &["smuggled"],
    )
    .await;
    for r in both(&ds, "lit").await {
        assert_eq!(r["aliases"], json!(["real"]), "{r}");
        let problems = r["aliases_problems"].to_string();
        assert!(problems.contains("smuggled"), "{r}");
    }
}

/// An unreadable property: no aliases (as org-roam gets none), and the reason.
#[tokio::test]
async fn malformed_roam_aliases_is_reported_not_guessed() {
    let ds = seed(
        "lit",
        Some(BODY_FORMAT_ORG_LITERAL_V1),
        "ok \"never closed",
        &[],
    )
    .await;
    for r in both(&ds, "lit").await {
        assert_eq!(r["aliases"], json!([]), "{r}");
        let problems = r["aliases_problems"].to_string();
        assert!(problems.contains("unterminated"), "{r}");
    }
}
