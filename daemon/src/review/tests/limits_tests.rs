//! E9's caps on the PR-4 writer methods: how many members a request names,
//! and how long each free-text parameter may be. Over a cap is refused
//! `invalid_params` with the cap in `data.limit`, and writes nothing; at the
//! cap is accepted. Run with the daemon's default scan limit (500), so the
//! member cap is shown to be its own and not the scan limit's. Kills a cap
//! that is missing, off by one, or counts characters instead of bytes.

use serde_json::json;

use super::*;

/// Call `method` as `who` under the daemon's default limits.
async fn call_default(
    env: &Env,
    who: &str,
    method: &str,
    params: Value,
) -> Result<Value, McpError> {
    crate::review::dispatch(
        method,
        &params,
        &env.store,
        &person(who),
        KbQueryLimits::default(),
    )
    .await
}

/// The cap a refusal names, after checking it is `invalid_params` on `param`.
fn limit_of(r: Result<Value, McpError>, param: &str) -> u64 {
    let e = r.expect_err("a refusal");
    let d = e.data.as_deref().unwrap();
    assert_eq!(
        (d["kind"].as_str(), d["param"].as_str()),
        (Some("invalid_params"), Some(param))
    );
    d["limit"].as_u64().expect("the cap in data.limit")
}

/// Add nodes `w0..w{n}` to the env's KB; returns `[{node_id, version}]`.
async fn wide(env: &Env, n: usize) -> Vec<Value> {
    let kbc = format!("kbc:{KB}");
    let state = env.store.encode_state(&kbc).await.unwrap();
    let mut coll = KbCollectionDoc::from_bytes(&state).unwrap();
    let by = Attribution::Principal("seed");
    let mut out = Vec::new();
    for i in 0..n {
        let id = format!("w{i}");
        let up = coll.add_node(&id, &id);
        env.store.apply_update(&kbc, &up, None, by).await.unwrap();
        let node = KbNodeDoc::new(&id, "T", "B", &[]);
        let name = mae_sync::kb_node_doc_name(KB, &id);
        env.store
            .share_doc(&name, &node.encode_state(), by)
            .await
            .unwrap();
        out.push(json!({"node_id": id, "version": node.version()}));
    }
    out
}

#[tokio::test]
async fn a_request_names_at_most_fifty_members() {
    let env = env().await;
    enable(&env).await;
    let mut nodes = wide(&env, 51).await;
    let ask = |nodes: &[Value]| json!({"kb_id": KB, "nodes": nodes, "reason": "r"});
    let r = call_default(&env, &env.editor, "kb/review.request", ask(&nodes)).await;
    assert_eq!(limit_of(r, "nodes"), 50);
    let none = env
        .store
        .review_read(|c| crate::review::store::has_revisions(c, KB))
        .await
        .unwrap();
    assert!(!none, "a refused request froze something");
    nodes.pop();
    let r = call_default(&env, &env.editor, "kb/review.request", ask(&nodes)).await;
    assert_eq!(r.unwrap()["revisions"].as_array().unwrap().len(), 50);
}

#[tokio::test]
async fn every_free_text_parameter_is_capped_in_bytes() {
    let env = env().await;
    enable(&env).await;
    let v = version(&env, "n1").await;
    // Two bytes a character: a character count would let these through.
    let text = |bytes: usize| format!("{}{}", "é".repeat(bytes / 2), "x".repeat(bytes % 2));
    let ask = |extra: Value| {
        let mut p = json!({"kb_id": KB, "nodes": [{"node_id": "n1", "version": v}], "reason": "r"});
        p.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        p
    };
    let (ed, own) = (env.editor.clone(), env.owner.clone());
    let over = [
        (
            &ed,
            "kb/review.request",
            ask(json!({"reason": text(2_049)})),
            "reason",
            2_048,
        ),
        (
            &ed,
            "kb/review.request",
            ask(json!({"set_name": text(201)})),
            "set_name",
            200,
        ),
        (
            &own,
            "kb/review.set_policy",
            json!({"kb_id": KB, "approval_record": "signed_pdf", "reason": text(2_049)}),
            "reason",
            2_048,
        ),
        (
            &own,
            "kb/review.set_policy",
            json!({"kb_id": KB, "code_prefixes": (0..33).map(|i| format!("P{i}")).collect::<Vec<_>>()}),
            "code_prefixes",
            32,
        ),
    ];
    for (who, method, params, param, cap) in over {
        assert_eq!(
            limit_of(call(&env, who, method, params).await, param),
            cap,
            "{method} {param}"
        );
    }
    let r = call(
        &env,
        &ed,
        "kb/review.request",
        ask(json!({"reason": text(2_048), "set_name": text(200)})),
    )
    .await
    .unwrap();
    let w = json!({"kb_id": KB, "request_id": r["request_id"], "reason": text(2_049)});
    assert_eq!(
        limit_of(call(&env, &ed, "kb/review.withdraw", w).await, "reason"),
        2_048
    );
    let w = json!({"kb_id": KB, "request_id": r["request_id"], "reason": text(2_048)});
    call(&env, &ed, "kb/review.withdraw", w).await.unwrap();
}
