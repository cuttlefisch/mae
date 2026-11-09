//! `kb/query.*` parameter validation: a parameter that is missing, of the
//! wrong type or out of range is refused with JSON-RPC -32602 and
//! `data: {kind: "invalid_params", param}`, never read as absent.
//!
//! The KB is real and the caller a member, so a lenient reader would answer
//! each of these with a result (a numeric `after` used to return the first
//! page again, which made a buggy poller loop forever without an error).

use serde_json::{json, Value};

use mae_daemon::kb_query::{self, KbQueryLimits};
use mae_mcp::protocol::INVALID_PARAMS;

use super::kb_query_tests::{fresh_doc_store, seed_listed_kb, LIST_VIEWER};

const KB: &str = "params-kb";

fn limits() -> KbQueryLimits {
    KbQueryLimits {
        max_body_bytes: 65_536,
        max_scan_nodes: 50,
        max_search_results: 20,
    }
}

#[tokio::test]
async fn every_malformed_parameter_is_refused_naming_it() {
    let doc_store = fresh_doc_store().await;
    seed_listed_kb(&doc_store, KB, 3, &[]).await;
    let cases: &[(&str, Value, &str)] = &[
        ("kb/query.list", json!({}), "kb_id"),
        ("kb/query.list", json!({"kb_id": 7}), "kb_id"),
        ("kb/query.list", json!({"kb_id": KB, "after": 5}), "after"),
        (
            "kb/query.list",
            json!({"kb_id": KB, "after": {"id": "n1"}}),
            "after",
        ),
        (
            "kb/query.list",
            json!({"kb_id": KB, "limit": "10"}),
            "limit",
        ),
        ("kb/query.list", json!({"kb_id": KB, "limit": -1}), "limit"),
        ("kb/query.list", json!({"kb_id": KB, "limit": 0}), "limit"),
        ("kb/query.list", json!({"kb_id": KB, "limit": 1.5}), "limit"),
        ("kb/query.get", json!({"kb_id": KB}), "node_id"),
        (
            "kb/query.get",
            json!({"kb_id": KB, "node_id": 1}),
            "node_id",
        ),
        ("kb/query.search", json!({"kb_id": KB}), "query"),
        (
            "kb/query.search",
            json!({"kb_id": KB, "query": "Node", "limit": "5"}),
            "limit",
        ),
        (
            "kb/query.search",
            json!({"kb_id": KB, "query": "Node", "limit": -2}),
            "limit",
        ),
        (
            "kb/query.links",
            json!({"kb_id": KB, "node_id": "n1", "direction": 3}),
            "direction",
        ),
        (
            "kb/query.links",
            json!({"kb_id": KB, "node_id": "n1", "direction": "sideways"}),
            "direction",
        ),
        (
            "kb/query.neighborhood",
            json!({"kb_id": KB, "node_id": "n1", "depth": "2"}),
            "depth",
        ),
        (
            "kb/query.titles",
            json!({"kb_id": KB, "prefix": true}),
            "prefix",
        ),
        (
            "kb/query.agenda",
            json!({"kb_id": KB, "filter": "custom"}),
            "filter",
        ),
        (
            "kb/query.agenda",
            json!({"kb_id": KB, "filter": "tag", "value": 1}),
            "value",
        ),
    ];
    for (method, params, param) in cases {
        let got = kb_query::dispatch(method, params, &doc_store, Some(LIST_VIEWER), limits()).await;
        let err = match got {
            Err(e) => e,
            Ok(v) => panic!("{method} {params} was answered: {v}"),
        };
        assert_eq!(err.code, INVALID_PARAMS, "{method} {params}: {err:?}");
        let data = err.data.as_deref().unwrap_or(&Value::Null);
        assert_eq!(data["kind"], "invalid_params", "{method} {params}");
        assert_eq!(data["param"], *param, "{method} {params}");
    }
}

/// `null` is no cursor, so a client can pass `next_after` back verbatim; it
/// returns the same first page as omitting `after`.
#[tokio::test]
async fn a_null_after_reads_as_the_first_page() {
    let doc_store = fresh_doc_store().await;
    seed_listed_kb(&doc_store, KB, 3, &[]).await;
    let page = |params: Value| {
        let doc_store = &doc_store;
        async move {
            kb_query::dispatch(
                "kb/query.list",
                &params,
                doc_store,
                Some(LIST_VIEWER),
                limits(),
            )
            .await
            .unwrap()
        }
    };
    let omitted = page(json!({"kb_id": KB, "limit": 2})).await;
    let null = page(json!({"kb_id": KB, "limit": 2, "after": null})).await;
    assert_eq!(omitted["nodes"][0]["id"], "n1");
    assert_eq!(null, omitted);
}

/// Below 1, a search `limit` and a neighbourhood `depth` are still clamped
/// rather than refused: the editor's hub client passes its caller's values
/// through and documents the clamp, so refusing 0 would break it.
#[tokio::test]
async fn a_zero_search_limit_or_depth_is_still_clamped() {
    let doc_store = fresh_doc_store().await;
    seed_listed_kb(&doc_store, KB, 3, &[]).await;
    let call = |method: &'static str, params: Value| {
        let doc_store = &doc_store;
        async move { kb_query::dispatch(method, &params, doc_store, Some(LIST_VIEWER), limits()).await }
    };
    let hits = call(
        "kb/query.search",
        json!({"kb_id": KB, "query": "Node", "limit": 0}),
    )
    .await
    .expect("search with limit 0");
    assert_eq!(hits["results"].as_array().unwrap().len(), 1);
    call(
        "kb/query.neighborhood",
        json!({"kb_id": KB, "node_id": "n1", "depth": 0}),
    )
    .await
    .expect("neighborhood with depth 0");
}
