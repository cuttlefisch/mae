//! D1b — `kb/query.agenda` and `kb/query.health`, the last three CLOSEABLE
//! declared gaps in `RemoteHubQueryLayer` (Agenda, TodoNodes, HealthReport).
//!
//! Tested at the `kb_query::dispatch` level with a real `DocStore` and real
//! `KbNodeDoc`s, matching this directory's existing convention.
//!
//! The adversarial focus is **truncation honesty**. Both endpoints are O(N) over
//! the corpus with no index behind them, so a capped answer is the normal case.
//! A capped agenda that reads as complete says "nothing is due", and a capped
//! health report that names orphans says a node is unreferenced when its only
//! backlink simply lay past the cap. Both are *wrong* answers, not partial ones.

use std::sync::Arc;

use mae_daemon::doc_store::DocStore;
use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, TransportPolicy};
use serde_json::json;

use mae_daemon::kb_query::{self, KbQueryLimits};

use super::kb_query_tests::{fresh_doc_store, generous_limits};

struct Seed<'a> {
    id: &'a str,
    title: &'a str,
    todo: Option<&'a str>,
    priority: Option<&'a str>,
    tags: &'a [&'a str],
    links: &'a [&'a str],
}

/// Build an unencrypted KB with several nodes carrying real agenda fields.
async fn seed_kb(doc_store: &DocStore, owner: &Arc<Identity>, kb_id: &str, seeds: &[Seed<'_>]) {
    doc_store.set_signer(Arc::clone(owner));
    let mut coll = KbCollectionDoc::new_owned(kb_id, &owner.fingerprint(), "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    for s in seeds {
        let _ = coll.add_node(s.id, s.title);
    }
    doc_store
        .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state())
        .await
        .unwrap();

    for s in seeds {
        let tags: Vec<String> = s.tags.iter().map(|t| t.to_string()).collect();
        let mut node = KbNodeDoc::new(s.id, s.title, "body", &tags);
        // The updates are discarded deliberately: the doc is published below via
        // `encode_state()`, i.e. as whole state, not as incremental ops — the
        // same shape `seed_unencrypted_kb` uses for links.
        let _ = node.set_todo_state(s.todo);
        let _ = node.set_priority(s.priority);
        for l in s.links {
            let _ = node.add_link(l);
        }
        doc_store
            .share_doc(
                &mae_sync::kb_node_doc_name(kb_id, s.id),
                &node.encode_state(),
            )
            .await
            .unwrap();
    }
}

fn corpus() -> Vec<Seed<'static>> {
    vec![
        Seed {
            id: "task:ship",
            title: "Ship it",
            todo: Some("TODO"),
            priority: Some("A"),
            tags: &["release"],
            links: &["note:hub"],
        },
        Seed {
            id: "task:wait",
            title: "Waiting on review",
            todo: Some("WAITING"),
            priority: Some("C"),
            tags: &["release", "blocked"],
            links: &["note:hub"],
        },
        Seed {
            id: "note:hub",
            title: "Hub note",
            todo: None,
            priority: None,
            tags: &[],
            links: &[],
        },
        Seed {
            id: "note:lonely",
            title: "Nothing points here",
            todo: None,
            priority: None,
            tags: &[],
            links: &[],
        },
    ]
}

async fn agenda(doc_store: &DocStore, filter: &str, value: Option<&str>) -> serde_json::Value {
    let mut params = json!({"kb_id": "agenda-kb", "filter": filter});
    if let Some(v) = value {
        params["value"] = json!(v);
    }
    kb_query::dispatch(
        "kb/query.agenda",
        &params,
        doc_store,
        None,
        generous_limits(),
    )
    .await
    .expect("agenda dispatch succeeds")
}

fn ids(result: &serde_json::Value) -> Vec<String> {
    let mut v: Vec<String> = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn every_served_agenda_filter_selects_the_right_nodes() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    seed_kb(&doc_store, &owner, "agenda-kb", &corpus()).await;

    assert_eq!(
        ids(&agenda(&doc_store, "todo", None).await),
        vec!["task:ship", "task:wait"],
        "todo with no state means ANY todo state"
    );
    assert_eq!(
        ids(&agenda(&doc_store, "todo", Some("WAITING")).await),
        vec!["task:wait"]
    );
    assert_eq!(
        ids(&agenda(&doc_store, "tag", Some("blocked")).await),
        vec!["task:wait"]
    );
    // Org priority runs A > B > C, so ASCII-ascending is DESCENDING urgency:
    // "at least B" must include A and exclude C. Getting this backwards is the
    // kind of bug that shows a plausible, wrong agenda every day.
    assert_eq!(
        ids(&agenda(&doc_store, "priority", Some("B")).await),
        vec!["task:ship"],
        "priority >= B includes A and excludes C"
    );
    assert_eq!(
        ids(&agenda(&doc_store, "dead-end", None).await),
        vec!["note:hub", "note:lonely"],
        "dead-end is 'no OUTgoing links', regardless of backlinks"
    );
    assert_eq!(
        ids(&agenda(&doc_store, "orphan", None).await),
        vec!["note:lonely"],
        "an orphan has neither direction — note:hub has backlinks and must not qualify"
    );
}

/// **`custom` Datalog is refused at the endpoint, not merely unimplemented.**
/// C3 established arbitrary Datalog is a privileged capability, and this surface
/// is served from the CRDT DocStore with no Datalog engine behind it at all.
#[tokio::test]
async fn a_custom_datalog_filter_is_refused_by_the_endpoint() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    seed_kb(&doc_store, &owner, "agenda-kb", &corpus()).await;

    let err = kb_query::dispatch(
        "kb/query.agenda",
        &json!({"kb_id": "agenda-kb", "filter": "custom", "value": "?[x] := *nodes{id: x}"}),
        &doc_store,
        None,
        generous_limits(),
    )
    .await
    .expect_err("an unsupported filter must be an ERROR, never an empty result");

    let msg = format!("{err:?}");
    assert!(
        msg.contains("custom"),
        "the refusal must name what was refused: {msg}"
    );
}

/// A capped agenda must SAY it is capped. A short list that claims completeness
/// reads as "nothing is due".
#[tokio::test]
async fn a_capped_agenda_reports_truncation() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    seed_kb(&doc_store, &owner, "agenda-kb", &corpus()).await;

    let tight = KbQueryLimits {
        max_scan_nodes: 2,
        ..generous_limits()
    };
    let result = kb_query::dispatch(
        "kb/query.agenda",
        &json!({"kb_id": "agenda-kb", "filter": "todo"}),
        &doc_store,
        None,
        tight,
    )
    .await
    .unwrap();

    assert_eq!(result["truncated"], json!(true));
    assert_eq!(result["scanned"], json!(2));
}

#[tokio::test]
async fn health_reports_the_shape_of_the_corpus_the_hub_holds() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    seed_kb(&doc_store, &owner, "agenda-kb", &corpus()).await;

    let result = kb_query::dispatch(
        "kb/query.health",
        &json!({"kb_id": "agenda-kb"}),
        &doc_store,
        None,
        generous_limits(),
    )
    .await
    .unwrap();

    assert_eq!(result["total_nodes"], json!(4));
    assert_eq!(result["total_links"], json!(2));
    assert_eq!(result["truncated"], json!(false));
    assert_eq!(
        result["orphan_ids"].as_array().unwrap(),
        &vec![json!("note:lonely")],
        "note:hub has two backlinks and is NOT an orphan"
    );
    assert_eq!(
        result["hub_nodes"][0],
        json!({"id": "note:hub", "in_degree": 2})
    );
    assert_eq!(result["namespace_counts"]["task"], json!(2));
    assert!(
        result["broken_links"].as_array().unwrap().is_empty(),
        "every link target exists"
    );
}

/// **The load-bearing one.** Under a cap, a node whose only backlink lies past
/// the scan looks orphaned and a link into the unscanned tail looks broken.
/// Both are withheld rather than reported wrongly.
#[tokio::test]
async fn a_capped_health_report_withholds_orphans_instead_of_inventing_them() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    seed_kb(&doc_store, &owner, "agenda-kb", &corpus()).await;

    let tight = KbQueryLimits {
        max_scan_nodes: 1,
        ..generous_limits()
    };
    let result = kb_query::dispatch(
        "kb/query.health",
        &json!({"kb_id": "agenda-kb"}),
        &doc_store,
        None,
        tight,
    )
    .await
    .unwrap();

    assert_eq!(result["truncated"], json!(true));
    assert!(
        result["orphan_ids"].as_array().unwrap().is_empty(),
        "a partial scan cannot know what is orphaned: {result}"
    );
    assert!(
        result["broken_links"].as_array().unwrap().is_empty(),
        "nor what is broken — the target may simply be past the cap: {result}"
    );
    assert_eq!(
        result["total_nodes"],
        json!(4),
        "the manifest count is still exact — it does not need the scan"
    );
}
