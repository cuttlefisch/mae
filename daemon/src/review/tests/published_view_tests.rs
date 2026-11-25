//! ADR-113 D5's `view: "published"` on `kb/query.get` and `kb/query.list`:
//! each node's in-force revision, from its frozen source. Each test names the
//! guard it kills.

use serde_json::{json, Value};

use super::act_tests::{approver, as_, policy, NOW};
use super::*;

async fn get(env: &Env, node: &str, view: &str) -> Result<Value, McpError> {
    let p = json!({"kb_id": KB, "node_id": node, "view": view});
    call(env, &env.viewer, "kb/query.get", p).await
}

async fn list(env: &Env, view: &str, after: Option<&str>, limit: u64) -> Value {
    let p = json!({"kb_id": KB, "view": view, "after": after, "limit": limit});
    call(env, &env.viewer, "kb/query.list", p).await.unwrap()
}

fn ids(page: &Value) -> Vec<String> {
    page["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect()
}

/// The editor edits `nodes`, requests them as one request, the approver
/// approves each; the request id.
async fn approved(env: &Env, nodes: &[&str]) -> String {
    for n in nodes {
        retitle(env, n, &format!("Approved {n}")).await;
    }
    let r = request(env, &env.editor, nodes).await.unwrap();
    for rev in r["revisions"].as_array().unwrap() {
        let p = json!({"kb_id": KB, "revision_id": rev["revision_id"]});
        as_(env, &approver(env), "kb/review.approve", p)
            .await
            .unwrap();
    }
    r["request_id"].as_str().unwrap().to_string()
}

async fn into_force(env: &Env, request_id: &str) {
    let p = json!({"kb_id": KB, "request_id": request_id, "effective_at": NOW});
    as_(env, &approver(env), "kb/review.enter_into_force", p)
        .await
        .unwrap();
}

/// Approved is not yet published; in force is; a later edit leaves the
/// published body and version alone while the working view moves; derogation
/// takes the node out of the view. Kills: serving the working copy, serving
/// an approved revision before entry into force, and a derogated one after.
#[tokio::test]
async fn the_published_view_is_the_in_force_revision_and_nothing_else() {
    let env = env().await;
    policy(&env, json!({})).await;
    let rq = approved(&env, &["n1"]).await;
    assert_eq!(
        err_kind(get(&env, "n1", "published").await),
        "node_not_in_kb"
    );
    assert!(ids(&list(&env, "published", None, 10).await).is_empty());

    into_force(&env, &rq).await;
    let p = get(&env, "n1", "published").await.unwrap();
    assert_eq!(p["title"], json!("Approved n1"));
    assert_eq!(p["revision"]["state"], json!("in_force"));
    let version = p["version"].clone();

    retitle(&env, "n1", "A later draft").await;
    let w = get(&env, "n1", "working").await.unwrap();
    assert_eq!(w["title"], json!("A later draft"));
    let p = get(&env, "n1", "published").await.unwrap();
    assert_eq!(
        (p["title"].clone(), p["version"].clone()),
        (json!("Approved n1"), version.clone())
    );
    let page = list(&env, "published", None, 10).await;
    assert_eq!(ids(&page), ["n1"]);
    assert_eq!(page["nodes"][0]["title"], json!("Approved n1"));
    assert_eq!(page["nodes"][0]["version"], version);
    assert_eq!(page["nodes"][0]["revision"]["state"], json!("in_force"));
    assert_eq!(ids(&list(&env, "working", None, 10).await).len(), 3);

    let d = json!({"kb_id": KB, "node_id": "n1", "effective_at": NOW, "reason": "Retired."});
    as_(&env, &approver(&env), "kb/review.derogate", d)
        .await
        .unwrap();
    assert_eq!(
        err_kind(get(&env, "n1", "published").await),
        "node_not_in_kb"
    );
    assert!(ids(&list(&env, "published", None, 10).await).is_empty());
}

/// The published list pages in node-id order like the working list. Kills a
/// page that drops or repeats a node, and a `next_after` on the last page.
#[tokio::test]
async fn the_published_list_pages_in_node_order() {
    let env = env().await;
    policy(&env, json!({})).await;
    let rq = approved(&env, &["n3", "n1", "n2"]).await;
    into_force(&env, &rq).await;
    let first = list(&env, "published", None, 2).await;
    assert_eq!(ids(&first), ["n1", "n2"]);
    assert_eq!(first["next_after"], json!("n2"));
    let rest = list(&env, "published", Some("n2"), 2).await;
    assert_eq!(ids(&rest), ["n3"]);
    assert_eq!(rest["next_after"], Value::Null);
}

/// With review off the published view is the working copy, and a `view`
/// other than the two is refused on both methods. Kills a published view
/// that is empty with review off, and one that ignores a mistyped view.
#[tokio::test]
async fn with_review_off_it_is_the_working_copy_and_a_bad_view_is_refused() {
    let env = env().await;
    retitle(&env, "n1", "Working title").await;
    assert_eq!(
        get(&env, "n1", "published").await.unwrap(),
        get(&env, "n1", "working").await.unwrap()
    );
    assert_eq!(
        list(&env, "published", None, 10).await,
        list(&env, "working", None, 10).await
    );
    assert_eq!(err_kind(get(&env, "n1", "draft").await), "invalid_params");
    let p = json!({"kb_id": KB, "view": 7});
    assert_eq!(
        err_kind(call(&env, &env.viewer, "kb/query.list", p).await),
        "invalid_params"
    );
}

/// The wire contract mae-web builds against: `query_published.json` (and the
/// acts of `query_get_revision.json`) have the real answers' field names, JSON
/// types and refusal kinds. Kills a drift between the fixture and the daemon.
#[tokio::test]
async fn the_published_view_answers_in_its_fixtures_shape() {
    let env = env().await;
    policy(&env, json!({})).await;
    let rq = approved(&env, &["n1", "n2"]).await;
    into_force(&env, &rq).await;
    let f = fixture("query_published");
    let ex = |i: usize| f["examples"][i]["response"]["result"].clone();
    shape(&ex(0), &get(&env, "n1", "published").await.unwrap(), "get");
    shape(&ex(1), &list(&env, "published", None, 1).await, "list");
    for (i, r) in [
        get(&env, "n3", "published").await,
        get(&env, "n1", "draft").await,
    ]
    .into_iter()
    .enumerate()
    {
        let want = &f["refusals"][i]["response"]["error"]["data"];
        assert_eq!(*r.unwrap_err().data.unwrap(), *want, "refusal {i}");
    }
    let acts =
        &fixture("query_get_revision")["examples"][0]["response"]["result"]["revision"]["acts"];
    let real = get(&env, "n1", "published").await.unwrap();
    assert_eq!(acts[0]["act"], real["revision"]["acts"][0]["act"]);
    shape(&acts[0], &real["revision"]["acts"][0], "acts");
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../docs/verification/fixtures/review-api/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// `actual` has `expected`'s keys, recursively, with the same JSON types
/// (`null` on either side matches anything).
fn shape(expected: &Value, actual: &Value, at: &str) {
    match (expected, actual) {
        (Value::Null, _) | (_, Value::Null) => {}
        (Value::Object(e), Value::Object(a)) => {
            let (mut ek, mut ak): (Vec<_>, Vec<_>) = (e.keys().collect(), a.keys().collect());
            ek.sort();
            ak.sort();
            assert_eq!(ek, ak, "{at}: keys");
            for (k, v) in e {
                shape(v, &a[k], &format!("{at}.{k}"));
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            if let Some(first) = e.first() {
                a.iter().for_each(|v| shape(first, v, &format!("{at}[]")));
            }
        }
        (e, a) => assert_eq!(
            std::mem::discriminant(e),
            std::mem::discriminant(a),
            "{at}: {e} vs {a}"
        ),
    }
}
