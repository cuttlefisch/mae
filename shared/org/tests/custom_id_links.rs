//! `[[id:X::#CUSTOM_ID]]` (ADR-119 S0, D5): the link lands on that CUSTOM_ID's
//! anchor when the page X resolves to has it, and on X's page otherwise:
//! never a placeholder, never a failure. A page's render reports where each
//! of its CUSTOM_IDs landed, and a resolver built from that report is what
//! another page's links use.

use mae_org::{
    parse, render, semantics_with, HeadlineIds, ParseOptions, RenderOptions, Rendered, Resolved,
    Resolver, SemanticsOptions,
};

const TARGET: &str = "\
:PROPERTIES:
:ID: x
:END:
#+title: X

* Plain
:PROPERTIES:
:CUSTOM_ID: plain
:END:
* Both a node and a custom id
:PROPERTIES:
:ID: both-node
:CUSTOM_ID: both
:END:
* Kept out of the export :noexport:
:PROPERTIES:
:CUSTOM_ID: hidden
:END:
";

fn rendered(text: &str, ids: HeadlineIds, resolver: &Resolver) -> Rendered {
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics_with(&doc, &SemanticsOptions::default());
    let opts = RenderOptions::default().with_headline_ids(ids);
    render(&doc, &sem, &opts, resolver)
}

fn html(r: &Rendered) -> String {
    mae_org::to_html(&r.body)
}

/// The href of the one link whose text is `text`.
fn href_of(body: &str, text: &str) -> String {
    let end = body
        .find(&format!(">{text}</a>"))
        .unwrap_or_else(|| panic!("no link {text:?}: {body}"));
    let start = body[..end].rfind("<a ").unwrap();
    let a = &body[start..end];
    let h = a.find("href=\"").unwrap() + 6;
    a[h..h + a[h..].find('"').unwrap()].to_string()
}

/// The resolver another page needs for X, built from X's own render.
fn resolver_for_target(ids: HeadlineIds) -> (Resolver, Rendered) {
    let target = rendered(TARGET, ids, &Resolver::default());
    let mut x = Resolved::new("/x.html").with_title("X");
    for (c, a) in &target.custom_ids {
        x = x.with_custom_id(c.clone(), a.clone());
    }
    (Resolver::default().with_target("x", x), target)
}

const SOURCE: &str = "\
:PROPERTIES:
:ID: src
:END:
[[id:x::#plain][to plain]] [[id:x::#both][to both]] [[id:x::#absent][to absent]] [[id:x][to x]]
";

#[test]
fn a_page_reports_where_each_rendered_custom_id_landed() {
    let (_, target) = resolver_for_target(HeadlineIds::Explicit);
    // `both` is anchored at its node id, not at a `cid-` id; the
    // `:noexport:` headline is not rendered, so its CUSTOM_ID is absent.
    assert_eq!(
        target.custom_ids,
        vec![
            ("both".to_string(), "id-both-node".to_string()),
            ("plain".to_string(), "cid-plain".to_string()),
        ]
    );
    let body = html(&target);
    for (_, anchor) in &target.custom_ids {
        assert!(
            body.contains(&format!("id=\"{anchor}\"")),
            "{anchor} not on the page: {body}"
        );
    }
    let json = target.to_json(mae_org::TreeFormat::Hast);
    assert!(
        json.ends_with(r#""custom_ids":[{"custom_id":"both","anchor":"id-both-node"},{"custom_id":"plain","anchor":"cid-plain"}]}"#),
        "{json}"
    );
}

#[test]
fn a_link_to_another_page_lands_on_the_custom_id_or_on_the_page() {
    let (resolver, _) = resolver_for_target(HeadlineIds::Explicit);
    let r = rendered(SOURCE, HeadlineIds::Explicit, &resolver);
    let body = html(&r);
    assert_eq!(href_of(&body, "to plain"), "/x.html#cid-plain");
    assert_eq!(href_of(&body, "to both"), "/x.html#id-both-node");
    // Absent on the target: the page itself.
    assert_eq!(href_of(&body, "to absent"), "/x.html");
    assert_eq!(href_of(&body, "to x"), "/x.html");
    assert!(!body.contains("[?]"), "never a placeholder: {body}");
    assert!(
        r.diagnostics.is_empty(),
        "never a failure: {:?}",
        r.diagnostics
    );
}

#[test]
fn with_slug_ids_the_custom_id_lands_on_its_heading() {
    let (resolver, target) = resolver_for_target(HeadlineIds::Slug);
    let anchor = &target
        .custom_ids
        .iter()
        .find(|(c, _)| c == "plain")
        .unwrap()
        .1;
    assert!(anchor.starts_with("h-"), "{anchor}");
    let body = html(&rendered(SOURCE, HeadlineIds::Slug, &resolver));
    assert!(
        body.contains(&format!("href=\"/x.html#{anchor}\"")),
        "{body}"
    );
}

#[test]
fn a_link_on_the_target_page_itself_uses_the_page_s_own_anchors() {
    // A headline node of this page, which no resolver names: the page's own
    // table decides. (The page's file-level node comes from the resolver,
    // which an engine fills for every page, this one included.) Before the
    // first headline, so the links are in the page's own text.
    let text = TARGET.replacen(
        "#+title: X\n",
        "#+title: X\n[[id:both-node::#plain][here]] [[id:both-node::#absent][nowhere]]\n",
        1,
    );
    let body = html(&rendered(
        &text,
        HeadlineIds::Explicit,
        &Resolver::default(),
    ));
    assert_eq!(href_of(&body, "here"), "#cid-plain");
    // Absent: the node's own anchor, never dropped.
    assert_eq!(href_of(&body, "nowhere"), "#id-both-node");
}

#[test]
fn a_resolver_supplied_anchor_cannot_break_out_of_the_href() {
    let hostile = Resolved::new("/x.html").with_custom_id("plain", "a\" onmouseover=\"alert(1)");
    let resolver = Resolver::default().with_target("x", hostile);
    let body = html(&rendered(SOURCE, HeadlineIds::Explicit, &resolver));
    assert!(!body.contains("onmouseover=\""), "{body}");
    assert!(!body.contains("\" onmouseover"), "{body}");
    // An href that already has a fragment keeps it.
    let fixed = Resolved::new("/x.html#top").with_custom_id("plain", "cid-plain");
    let body = html(&rendered(
        SOURCE,
        HeadlineIds::Explicit,
        &Resolver::default().with_target("x", fixed),
    ));
    assert!(body.contains(r#"href="/x.html#top""#), "{body}");
}
