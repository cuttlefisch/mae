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
    r.targets
        .insert("a".into(), mae_org::Resolved::new("/a.html"));
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

/// The facade and the parser are versioned together, and neither inherits
/// MAE's workspace version (whose every release would otherwise change the
/// version the module reports, and so its bytes).
#[test]
fn the_module_reports_the_contract_version_it_is() {
    let v = ok(6, "");
    assert_eq!(
        v["api"], v["crate"],
        "mae-org and mae-org-wasm must move together"
    );
    assert_eq!(mae_org::API_VERSION, env!("CARGO_PKG_VERSION"));
    let workspace =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
            .expect("the workspace manifest");
    let mae = workspace
        .split("[workspace.package]")
        .nth(1)
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("version = ")))
        .expect("the workspace version");
    assert_ne!(
        mae.trim_matches('"'),
        mae_org::API_VERSION,
        "the contract version must not be MAE's workspace version"
    );
}

#[test]
fn options_reach_the_renderer() {
    let req = r##"{"text": "* Same\n* Same\n", "render": {"headline_ids": "slug"}}"##;
    let body = ok(3, req)["body"].as_str().unwrap().to_string();
    assert!(body.contains("id=\"h-same--2\""), "{body}");
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

/// `[kind name, start, end]` triples of a spans result.
fn triples(r: &Value) -> Vec<(String, u64, u64)> {
    let kinds = r["kinds"].as_array().expect("kinds");
    r["spans"]
        .as_array()
        .expect("spans")
        .chunks(3)
        .map(|c| {
            let k = kinds[c[0].as_u64().unwrap() as usize]
                .as_str()
                .unwrap()
                .to_string();
            (k, c[1].as_u64().unwrap(), c[2].as_u64().unwrap())
        })
        .collect()
}

const SYN: &str = "#+TODO: TODO | DONE\nintro <<<radio>>>\n* TODO A :t:\nalpha *b* radio\n** A1\nchild\n* B\nbeta\n";

/// Through JSON both ways, a region re-highlighted from its unchanged text
/// gives exactly the whole result's spans inside it; an edit that changes
/// what the rest of the file reads is answered with `needs_full`.
#[test]
fn syntax_sections_round_trip_through_json() {
    let whole = ok(7, &serde_json::json!({ "text": SYN }).to_string());
    let all = triples(&whole);
    assert!(all.iter().any(|t| t.0 == "TODO_KEYWORD"), "{all:?}");
    let sections = whole["sections"].as_array().unwrap();
    assert_eq!(sections.len(), 4);
    for s in sections {
        let (start, end) = (s["start"].as_u64().unwrap(), s["end"].as_u64().unwrap());
        let region = &SYN[start as usize..end as usize];
        let req = serde_json::json!({ "text": region, "section": s, "file": whole["file"] });
        let got = triples(&ok(8, &req.to_string()));
        let want: Vec<_> = all
            .iter()
            .filter(|t| t.1 >= start && t.2 <= end)
            .cloned()
            .collect();
        assert_eq!(got, want, "section {start}..{end}");
    }
    let child = sections
        .iter()
        .find(|s| SYN[s["start"].as_u64().unwrap() as usize..].starts_with("child"))
        .unwrap();
    let edited =
        serde_json::json!({ "text": "child\n* new\n", "section": child, "file": whole["file"] });
    assert_eq!(
        ok(8, &edited.to_string()),
        serde_json::json!({ "needs_full": "headline" })
    );
}

/// A headline whose title ends in a multi-byte character before its LF:
/// `syntax` answers, and each section round-trips through `syntaxSection`.
/// Each input made 0.5.0's `syntax` panic (`unreachable` in WASM).
#[test]
fn a_multibyte_character_before_a_headline_lf_is_answered() {
    for text in [
        "* ñ\n",
        "* €\n",
        "* 😀\n",
        "* 𝄞\n",
        "* a😀\n",
        "** 😀\n",
        "texto\n* 😀\n",
    ] {
        for post_blank in [true, false] {
            let req = serde_json::json!({ "text": text, "post_blank": post_blank });
            let whole = ok(7, &req.to_string());
            for s in whole["sections"].as_array().unwrap() {
                let (start, end) = (s["start"].as_u64().unwrap(), s["end"].as_u64().unwrap());
                let region = &text[start as usize..end as usize];
                let req = serde_json::json!({
                    "text": region, "section": s, "file": whole["file"], "post_blank": post_blank,
                });
                assert!(ok(8, &req.to_string())["spans"].is_array(), "{text:?}");
            }
        }
    }
}

#[test]
fn a_malformed_section_request_is_an_error() {
    assert!(err(8, r#"{"text": "x"}"#).contains("section"));
    let bad = r#"{"text": "x", "file": {"line_ending": "lf"}, "section": {"start": -1, "end": 1, "level": 0, "ancestors": 0, "declarations": 0, "followed": false}}"#;
    assert!(err(8, bad).contains("section.start"));
    let eol = r#"{"text": "x", "file": {"line_ending": "dos"}, "section": {"start": 0, "end": 1, "level": 0, "ancestors": 0, "declarations": 0, "followed": false}}"#;
    assert!(err(8, eol).contains("line_ending"));
    // `eol` is required (0.5.0): it is how a section edit learns it may
    // change the file's line ending.
    for bad_eol in ["", r#", "eol": 32"#, r#", "eol": -1"#] {
        let req = format!(
            r#"{{"text": "x", "file": {{"line_ending": "lf"}}, "section": {{"start": 0, "end": 1, "level": 0, "ancestors": 0, "declarations": 0, "followed": false{bad_eol}}}}}"#
        );
        assert!(err(8, &req).contains("section.eol"), "{bad_eol}");
    }
}

/// `"post_blank": false` ends each span before its post-blank, in `syntax`
/// and `syntaxSection` alike; the default keeps `org-element`'s ends; a
/// non-boolean is refused.
#[test]
fn post_blank_false_ends_spans_before_the_spaces_after_them() {
    let text = "un *negrita* y más\n\n* H\nver [[id:n1][Uno]] y\n";
    let find = |r: &Value, kind: &str| {
        triples(r)
            .into_iter()
            .find(|t| t.0 == kind)
            .unwrap_or_else(|| panic!("no {kind}"))
    };
    let default = ok(7, &serde_json::json!({ "text": text }).to_string());
    assert_eq!(find(&default, "BOLD"), ("BOLD".into(), 3, 13));
    let own = ok(
        7,
        &serde_json::json!({ "text": text, "post_blank": false }).to_string(),
    );
    assert_eq!(find(&own, "BOLD"), ("BOLD".into(), 3, 12));
    assert_eq!(find(&own, "PARAGRAPH"), ("PARAGRAPH".into(), 0, 20));

    let link_section = own["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| text[s["start"].as_u64().unwrap() as usize..].starts_with("ver"))
        .unwrap();
    let (start, end) = (
        link_section["start"].as_u64().unwrap() as usize,
        link_section["end"].as_u64().unwrap() as usize,
    );
    let req = |post_blank: bool| {
        serde_json::json!({
            "text": &text[start..end], "section": link_section,
            "file": own["file"], "post_blank": post_blank,
        })
        .to_string()
    };
    let link_at = start as u64 + 4;
    assert_eq!(
        find(&ok(8, &req(false)), "LINK"),
        ("LINK".into(), link_at, link_at + 14)
    );
    assert_eq!(
        find(&ok(8, &req(true)), "LINK"),
        ("LINK".into(), link_at, link_at + 15)
    );

    assert!(err(7, r#"{"text": "x", "post_blank": "no"}"#).contains("post_blank"));
}

/// `[[id:X::#C]]` through the WASM surface (ADR-119 S0): a target's
/// `custom_ids` come from that page's own render, and the link lands on the
/// anchor when `C` is there and on the page when it is not. Bad shapes are
/// refused, never ignored.
#[test]
fn a_resolver_target_carries_the_target_page_s_custom_ids() {
    let target = ok(
        2,
        r##"{"text": ":PROPERTIES:\n:ID: x\n:END:\n* P\n:PROPERTIES:\n:CUSTOM_ID: p\n:END:\n"}"##,
    );
    assert_eq!(target["custom_ids"][0]["custom_id"], "p");
    let anchor = target["custom_ids"][0]["anchor"]
        .as_str()
        .unwrap()
        .to_string();
    let req = format!(
        r##"{{"text": "[[id:x::#p][to p]] [[id:x::#q][to q]]\n",
             "resolver": {{"targets": {{"x": {{"href": "/x.html", "custom_ids": {{"p": "{anchor}"}}}}}}}}}}"##
    );
    // Op 2 is the hast render: each href is a JSON string in it.
    let got = ok(2, &req).to_string();
    assert!(got.contains(&format!("\"/x.html#{anchor}\"")), "{got}");
    assert!(
        got.contains("\"/x.html\""),
        "the absent one goes to the page: {got}"
    );
    assert!(err(
        2,
        r##"{"resolver": {"targets": {"x": {"href": "/x", "custom_ids": ["p"]}}}}"##
    )
    .contains("custom_ids"));
    assert!(err(
        2,
        r##"{"resolver": {"targets": {"x": {"href": "/x", "custom_ids": {"p": 1}}}}}"##
    )
    .contains("anchor"));
}

/// `resolver.images` (ADR-115 E3): each state reaches the renderer, and the
/// render is mae-org's own for the same resolver built natively.
#[test]
fn resolver_images_reach_the_renderer() {
    let text = "[[attachment:a.png]] [[attachment:b.png]] [[file:c.png]] [[attachment:d.png]] [[attachment:e.png]] [[attachment:f.png]]\n";
    let req = format!(
        r##"{{"text": {text:?}, "resolver": {{"images": {{
            "attachment:a.png": {{"state": "resolved", "url": "/b/a", "media_type": "image/png", "width": 10, "height": 20}},
            "attachment:b.png": {{"state": "resolved", "url": "/b/b", "media_type": "image/png", "width": null}},
            "file:c.png": {{"state": "missing"}},
            "attachment:d.png": {{"state": "forbidden"}},
            "attachment:e.png": {{"state": "too_large"}},
            "attachment:f.png": {{"state": "unsupported"}}}}}}}}"##
    );
    let html = ok(3, &req);
    let html = html["body"].as_str().unwrap();
    assert!(
        html.contains(r#"<img src="/b/a" alt="a.png" width="10" height="20">"#),
        "{html}"
    );
    assert!(html.contains(r#"<img src="/b/b" alt="b.png">"#), "{html}");
    for (name, state) in [
        ("c.png", "missing"),
        ("d.png", "forbidden"),
        ("e.png", "too_large"),
        ("f.png", "unsupported"),
    ] {
        let span = format!(r#"<span class="org-attachment" data-state="{state}">{name}</span>"#);
        assert!(html.contains(&span), "{span}: {html}");
    }
    // The same resolver built natively renders the same body.
    let doc = mae_org::parse(text, &mae_org::ParseOptions::default());
    let sem = mae_org::semantics(&doc);
    let png = |u: &str| mae_org::ResolvedImage::new(u, "image/png");
    let r = mae_org::Resolver::default()
        .with_image(
            "attachment:a.png",
            mae_org::ImageState::Resolved(png("/b/a").with_width(10).with_height(20)),
        )
        .with_image(
            "attachment:b.png",
            mae_org::ImageState::Resolved(png("/b/b")),
        )
        .with_image("file:c.png", mae_org::ImageState::Missing)
        .with_image("attachment:d.png", mae_org::ImageState::Forbidden)
        .with_image("attachment:e.png", mae_org::ImageState::TooLarge)
        .with_image("attachment:f.png", mae_org::ImageState::Unsupported);
    let native = mae_org::render_html(&doc, &sem, &mae_org::RenderOptions::default(), &r);
    assert_eq!(html, native);
}

#[test]
fn a_malformed_image_state_is_an_error() {
    let bad = |images: &str, needle: &str| {
        let e = err(2, &format!(r##"{{"resolver": {{"images": {images}}}}}"##));
        assert!(e.contains(needle), "{images}: {e}");
    };
    bad(r#"[]"#, "resolver.images");
    bad(r#"{"attachment:x.png": "missing"}"#, "must be an object");
    bad(r#"{"attachment:x.png": {}}"#, "no state");
    bad(r#"{"attachment:x.png": {"state": "gone"}}"#, "\"gone\"");
    bad(
        r#"{"attachment:x.png": {"state": "Missing"}}"#,
        "\"Missing\"",
    );
    bad(r#"{"attachment:x.png": {"state": 1}}"#, "state");
    // A placeholder state takes no other key.
    bad(
        r#"{"attachment:x.png": {"state": "missing", "url": "/x"}}"#,
        "\"url\"",
    );
    // A resolved one needs url and media_type, both strings, and nothing else.
    bad(
        r#"{"attachment:x.png": {"state": "resolved", "media_type": "image/png"}}"#,
        "no url",
    );
    bad(
        r#"{"attachment:x.png": {"state": "resolved", "url": "/x"}}"#,
        "no media_type",
    );
    bad(
        r#"{"attachment:x.png": {"state": "resolved", "url": 3, "media_type": "image/png"}}"#,
        "url",
    );
    bad(
        r#"{"attachment:x.png": {"state": "resolved", "url": "/x", "media_type": "image/png", "alt": "x"}}"#,
        "\"alt\"",
    );
    for w in ["-1", "1.5", "\"10\"", "4294967296", "1e400", "true"] {
        bad(
            &format!(
                r#"{{"attachment:x.png": {{"state": "resolved", "url": "/x", "media_type": "image/png", "width": {w}}}}}"#
            ),
            "width",
        );
        bad(
            &format!(
                r#"{{"attachment:x.png": {{"state": "resolved", "url": "/x", "media_type": "image/png", "height": {w}}}}}"#
            ),
            "height",
        );
    }
    // The largest size is accepted.
    let max = r##"{"resolver": {"images": {"attachment:x.png": {"state": "resolved", "url": "/x", "media_type": "image/png", "width": 4294967295}}}, "text": "[[attachment:x.png]]\n"}"##;
    assert!(ok(3, max)["body"]
        .as_str()
        .unwrap()
        .contains("width=\"4294967295\""));
}

/// An `attachment:` link's directory reaches the semantics JSON.
#[test]
fn the_semantics_carry_the_attachment_directory() {
    let req = r##"{"text": "* H\n:PROPERTIES:\n:ID: abcdef\n:END:\n[[attachment:x.png]] [[file:y.png]]\n"}"##;
    let links = ok(1, req)["semantics"]["links"].clone();
    assert_eq!(links[0]["kind"], "other");
    assert_eq!(links[0]["target"], "attachment:x.png");
    assert_eq!(links[0]["attach_dir"], "data/ab/cdef/");
    assert_eq!(links[0]["attach_id"], "abcdef");
    assert_eq!(links[1]["attach_dir"], Value::Null);
    assert_eq!(links[1]["attach_id"], Value::Null);
}
