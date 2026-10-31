//! The facade's operations, natively: every response is JSON with the
//! documented envelope; the results are mae-org's own (the facade adds no
//! rendering of its own); a malformed or misspelt request is an error
//! response, never a default.

use mae_org_wasm::api::call;
use serde_json::Value;

fn ok(op: u32, req: &str) -> Value {
    let r: Value = serde_json::from_str(&call(op, req)).expect("JSON response");
    assert_eq!(r["ok"], true, "{r}");
    r["result"].clone()
}

fn err(op: u32, req: &str) -> String {
    let r: Value = serde_json::from_str(&call(op, req)).expect("JSON response");
    assert_eq!(r["ok"], false, "{r}");
    r["error"].as_str().unwrap_or_default().to_string()
}

const DOC: &str = "#+title: T\n* H [[id:a][A]]\n:PROPERTIES:\n:ID: h\n:END:\nBody [[id:b]].\n";

#[test]
fn results_are_mae_orgs_own() {
    let req = r##"{"text": "#+title: T\n* H [[id:a][A]]\n:PROPERTIES:\n:ID: h\n:END:\nBody [[id:b]].\n",
                 "resolver": {"targets": {"a": {"href": "/a.html", "title": null}}}}"##;
    let doc = mae_org::parse(DOC, &mae_org::ParseOptions::default());
    let sem = mae_org::semantics(&doc);
    let mut r = mae_org::Resolver::default();
    r.targets.insert(
        "a".into(),
        mae_org::Resolved {
            href: "/a.html".into(),
            title: None,
        },
    );
    let want = mae_org::render(&doc, &sem, &mae_org::RenderOptions::default(), &r);
    let got = call(2, req);
    assert_eq!(
        got,
        format!(
            "{{\"ok\":true,\"result\":{}}}",
            want.to_json(mae_org::TreeFormat::Hast)
        )
    );
    let s = ok(1, req);
    assert_eq!(s["semantics"]["nodes"][0]["id"], "h");
    assert_eq!(s["semantics"]["links"][1]["target"], "b");
    assert_eq!(s["line_ending"], "lf");
    assert_eq!(ok(4, req), Value::String(mae_org::text(&doc, &sem)));
    let html = ok(3, req);
    assert!(html["body"].as_str().unwrap().contains("href=\"/a.html\""));
    assert_eq!(ok(5, r##"{"path": "id:x#f?rel=r"}"##)["rel"], "r");
    assert_eq!(ok(6, "")["api"], mae_org::API_VERSION);
}

#[test]
fn options_reach_the_renderer() {
    let req = r##"{"text": "* Same\n* Same\n", "render": {"headline_ids": "slug"}}"##;
    let body = ok(3, req)["body"].as_str().unwrap().to_string();
    assert!(body.contains("id=\"h-same-2\""), "{body}");
    let req =
        r##"{"text": "* x :secret:\nhidden\n* y\n", "semantics": {"exclude_tags": ["secret"]}}"##;
    assert!(!ok(4, req).as_str().unwrap().contains("hidden"));
    let req =
        r##"{"text": "* a\n:PROPERTIES:\n:ID: n\n:END:\nnode body\n* b\nother\n", "node": "n"}"##;
    assert_eq!(ok(4, req), "a\nnode body");
}

#[test]
fn bad_requests_are_error_responses() {
    assert!(err(2, "{").contains("invalid JSON"));
    assert!(err(2, r##"{"txt": "x"}"##).contains("unknown key \"txt\""));
    assert!(err(2, r##"{"render": {"imagse": true}}"##).contains("imagse"));
    assert!(err(2, r##"{"text": 1}"##).contains("text"));
    assert!(err(2, r##"{"resolver": {"targets": {"a": {"title": "x"}}}}"##).contains("href"));
    assert!(err(4, r##"{"text": "x", "node": "missing"}"##).contains("missing"));
    assert!(err(5, "{}").contains("path"));
    assert!(err(99, "{}").contains("unknown operation"));
    assert!(err(1, "[]").contains("object"));
    assert!(err(1, r##"{"parse": {"todo": "TODO DONE"}}"##).contains("parse.todo"));
}
