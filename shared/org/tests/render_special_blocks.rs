//! A special block's parameters reach the output as `data-org-params`: the
//! text after the block name, trimmed, as a plain attribute value. Only on a
//! block whose name is valid, only when non-empty, at most 512 bytes, and with
//! no control characters; otherwise the attribute is absent, never mangled.

use mae_org::{parse, render_hast, render_html, semantics, ParseOptions, RenderOptions, Resolver};

/// The `data-org-params` of the first special block in `text`, as rendered to
/// both HTML (escaped) and hast (raw).
fn params(text: &str) -> (Option<String>, Option<String>) {
    let doc = parse(text, &ParseOptions::default());
    let (sem, opts, res) = (
        semantics(&doc),
        RenderOptions::default(),
        Resolver::default(),
    );
    let html = render_html(&doc, &sem, &opts, &res);
    let in_html = html
        .split_once("data-org-params=\"")
        .map(|(_, rest)| rest.split_once('"').unwrap().0.to_string());
    let hast: serde_json::Value =
        serde_json::from_str(&render_hast(&doc, &sem, &opts, &res)).unwrap();
    let in_hast = find(&hast, "dataOrgParams");
    (in_html, in_hast)
}

fn find(v: &serde_json::Value, key: &str) -> Option<String> {
    match v {
        serde_json::Value::Object(m) => m
            .get(key)
            .and_then(|x| x.as_str().map(str::to_string))
            .or_else(|| m.values().find_map(|x| find(x, key))),
        serde_json::Value::Array(a) => a.iter().find_map(|x| find(x, key)),
        _ => None,
    }
}

fn block(begin: &str) -> String {
    format!("#+begin_{begin}\nbody\n#+end_details\n")
}

#[test]
fn parameters_are_the_trimmed_text_after_the_name() {
    let got = params(&block("details   En Windows  "));
    assert_eq!(got, (Some("En Windows".into()), Some("En Windows".into())));
    let quoted = params(&block("details a \"b\" & <c>"));
    assert_eq!(
        quoted.0.as_deref(),
        Some("a &quot;b&quot; &amp; &lt;c&gt;"),
        "escaped in HTML"
    );
    assert_eq!(quoted.1.as_deref(), Some("a \"b\" & <c>"), "raw in hast");
}

#[test]
fn the_attribute_is_absent_where_it_must_be() {
    assert_eq!(params(&block("details")), (None, None), "no parameters");
    assert_eq!(params(&block("details    ")), (None, None), "only spaces");
    assert_eq!(
        params(&block("details a\tb")),
        (None, None),
        "a control character"
    );
    let at_cap = "x".repeat(512);
    assert_eq!(params(&block(&format!("details {at_cap}"))).1, Some(at_cap));
    assert_eq!(
        params(&block(&format!("details {}", "x".repeat(513)))),
        (None, None),
        "over 512 bytes"
    );
    let bad_name = "#+begin_no!te p\nbody\n#+end_no!te\n";
    assert_eq!(
        params(bad_name),
        (None, None),
        "no data-org-block, so no params"
    );
}
