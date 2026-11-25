//! E12: the committed fixtures are the wire contract a client builds
//! against. Every method PR-4 serves answers with the fixture's field names
//! and JSON types (ids, hashes and times differ), and refuses with the
//! fixture's `data.kind` and `data` fields. `null` on either side matches any
//! type: a field that is null here until a later PR fills it (a performer, a
//! retention period) keeps its name.

use serde_json::json;

use super::*;

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../docs/verification/fixtures/review-api/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Every place `actual`'s shape differs from `expected`'s.
fn shape_diff(expected: &Value, actual: &Value, path: &str, out: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Null, _) | (_, Value::Null) => {}
        (Value::Object(e), Value::Object(a)) => {
            for k in e.keys().filter(|k| !a.contains_key(*k)) {
                out.push(format!("{path}.{k}: missing"));
            }
            for k in a.keys().filter(|k| !e.contains_key(*k)) {
                out.push(format!("{path}.{k}: not in the fixture"));
            }
            for (k, ev) in e {
                if let Some(av) = a.get(k) {
                    shape_diff(ev, av, &format!("{path}.{k}"), out);
                }
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            if let Some(first) = e.first() {
                for (i, av) in a.iter().enumerate() {
                    shape_diff(first, av, &format!("{path}[{i}]"), out);
                }
            }
        }
        (e, a) if type_of(e) != type_of(a) => out.push(format!(
            "{path}: {} in the fixture, {} here",
            type_of(e),
            type_of(a)
        )),
        _ => {}
    }
}

fn assert_shape(name: &str, expected: &Value, actual: &Value) {
    let mut out = Vec::new();
    shape_diff(expected, actual, name, &mut out);
    assert!(out.is_empty(), "{name}:\n{}", out.join("\n"));
}

fn example_result(f: &Value, i: usize) -> Value {
    f["examples"][i]["response"]["result"].clone()
}

/// The refusal named `case` in fixture `f`.
fn refusal(f: &Value, case: &str) -> Value {
    f["refusals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["case"] == case)
        .unwrap_or_else(|| panic!("no refusal {case:?}"))["response"]["error"]["data"]
        .clone()
}

fn data(r: Result<Value, McpError>) -> Value {
    let e = r.expect_err("a refusal");
    e.data.as_deref().unwrap().clone()
}

fn assert_refusal(name: &str, expected: &Value, actual: &Value) {
    assert_eq!(expected["kind"], actual["kind"], "{name}");
    assert_shape(name, expected, actual);
}

/// A KB with review on, codes, a three-node request open and a withdrawn
/// one, so every list and record the fixtures show has an entry.
async fn populated() -> (Env, Value) {
    let env = env().await;
    let policy = fixture("set_policy");
    let mut on = policy["examples"][0]["request"]["params"].clone();
    on["kb_id"] = json!(KB);
    for later in [
        "reviewers",
        "approvers",
        "approval_requires_mfa",
        "approval_requires_rendered_pdf",
        "reauth_max_age_secs",
        "attachment_retention_days",
    ] {
        on.as_object_mut().unwrap().remove(later);
    }
    let set = call(&env, &env.owner, "kb/review.set_policy", on)
        .await
        .unwrap();
    assert_shape("set_policy", &example_result(&policy, 0), &set);
    (env, policy)
}

#[tokio::test]
async fn every_pr4_method_answers_in_its_fixtures_shape() {
    let (env, _) = populated().await;

    let f = fixture("set_code");
    for (n, code) in [("n1", "PRO-1"), ("n2", "PRO-2"), ("n3", "PRO-3")] {
        let r = call(
            &env,
            &env.owner,
            "kb/review.set_code",
            json!({"kb_id": KB, "node_id": n, "doc_code": code}),
        )
        .await
        .unwrap();
        assert_shape("set_code", &example_result(&f, 0), &r);
    }
    let r = call(
        &env,
        &env.owner,
        "kb/review.set_code",
        json!({"kb_id": KB, "node_id": "n3", "doc_code": "PRO-2"}),
    )
    .await;
    assert_refusal(
        "set_code taken",
        &refusal(&f, "number already used for that prefix"),
        &data(r),
    );

    let f = fixture("request");
    let first = request(&env, &env.editor, &["n1"]).await.unwrap();
    let w = call(
        &env,
        &env.editor,
        "kb/review.withdraw",
        json!({"kb_id": KB, "request_id": first["request_id"], "reason": "r"}),
    )
    .await
    .unwrap();
    assert_shape("withdraw", &example_result(&fixture("withdraw"), 0), &w);
    let r = request(&env, &env.editor, &NODES).await.unwrap();
    assert_shape("request", &example_result(&f, 0), &r);

    let st = call(
        &env,
        &env.viewer,
        "kb/review.status",
        json!({"kb_id": KB, "node_id": "n1"}),
    )
    .await
    .unwrap();
    assert_shape("status", &example_result(&fixture("status"), 0), &st);
    assert_eq!(st["revisions"].as_array().unwrap().len(), 2);

    let p = call(
        &env,
        &env.viewer,
        "kb/review.pending",
        json!({"kb_id": KB, "limit": 50}),
    )
    .await
    .unwrap();
    assert_shape("pending", &example_result(&fixture("pending"), 0), &p);
    assert_eq!(p["requests"].as_array().unwrap().len(), 1);

    let ev = fixture("events");
    let page = call(
        &env,
        &env.owner,
        "kb/review.events",
        json!({"kb_id": KB, "limit": 2}),
    )
    .await
    .unwrap();
    assert_shape("events", &example_result(&ev, 0), &page);
    let last = call(
        &env,
        &env.owner,
        "kb/review.events",
        json!({"kb_id": KB, "limit": 2, "cursor": page["next_cursor"]}),
    )
    .await
    .unwrap();
    assert_shape("events page 2", &example_result(&ev, 1), &last);
}

#[tokio::test]
async fn every_pr4_refusal_carries_its_fixtures_kind_and_fields() {
    let (env, policy) = populated().await;
    let f = fixture("request");
    let v1 = version(&env, "n1").await;
    let ask = |extra: Value| {
        let mut p =
            json!({"kb_id": KB, "nodes": [{"node_id": "n1", "version": v1}], "reason": "x"});
        p.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        p
    };
    let stale = call(
        &env,
        &env.editor,
        "kb/review.request",
        ask(json!({"nodes": [{"node_id": "n1", "version": "0"}]})),
    )
    .await;
    assert_refusal(
        "stale",
        &refusal(&f, "the working copy moved on"),
        &data(stale),
    );
    let no_reason = call(
        &env,
        &env.editor,
        "kb/review.request",
        ask(json!({"reason": null})),
    )
    .await;
    assert_refusal("reason", &refusal(&f, "no reason given"), &data(no_reason));
    let no_node = call(
        &env,
        &env.editor,
        "kb/review.request",
        ask(json!({"nodes": [{"node_id": "ffffffff-0000-4000-8000-000000000000", "version": v1}]})),
    )
    .await;
    assert_refusal(
        "node",
        &refusal(&f, "node is not in this KB"),
        &data(no_node),
    );
    let loosen = call(
        &env,
        &env.editor,
        "kb/review.request",
        ask(json!({"in_force_on_approval": true})),
    )
    .await;
    assert_refusal(
        "loosen",
        &refusal(&f, "requester loosens in_force_on_approval"),
        &data(loosen),
    );
    let stranger = call(&env, &env.stranger, "kb/review.request", ask(json!({}))).await;
    assert_refusal(
        "member",
        &refusal(&f, "caller is not a member (identical for an unknown KB)"),
        &data(stranger),
    );

    let not_owner = call(
        &env,
        &env.editor,
        "kb/review.set_policy",
        json!({"kb_id": KB, "review_enabled": false}),
    )
    .await;
    assert_refusal(
        "set_policy",
        &refusal(&policy, "not the owner"),
        &data(not_owner),
    );

    let bad_cursor = call(
        &env,
        &env.viewer,
        "kb/review.pending",
        json!({"kb_id": KB, "cursor": "not-a-cursor"}),
    )
    .await;
    assert_refusal(
        "pending",
        &refusal(&fixture("pending"), "unknown cursor"),
        &data(bad_cursor),
    );

    let not_owner = call(&env, &env.editor, "kb/review.events", json!({"kb_id": KB})).await;
    assert_refusal(
        "events",
        &refusal(&fixture("events"), "a member who is not owner or auditor"),
        &data(not_owner),
    );

    let no_node = call(
        &env,
        &env.viewer,
        "kb/review.status",
        json!({"kb_id": KB, "node_id": "nope"}),
    )
    .await;
    assert_refusal(
        "status",
        &refusal(&fixture("status"), "unknown node"),
        &data(no_node),
    );

    let r = request(&env, &env.editor, &["n2"]).await.unwrap();
    let w = json!({"kb_id": KB, "request_id": r["request_id"], "reason": "x"});
    call(&env, &env.editor, "kb/review.withdraw", w.clone())
        .await
        .unwrap();
    let again = data(call(&env, &env.editor, "kb/review.withdraw", w).await);
    assert_refusal(
        "withdraw",
        &refusal(&fixture("withdraw"), "after entry into force"),
        &again,
    );
    assert_eq!(again["from"], json!("withdrawn"));

    // review off, last: it is the one fixture case that needs the switch off.
    call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        json!({"kb_id": KB, "review_enabled": false}),
    )
    .await
    .unwrap();
    let off = call(&env, &env.editor, "kb/review.request", ask(json!({}))).await;
    assert_refusal("off", &refusal(&f, "review is off for this KB"), &data(off));
}
