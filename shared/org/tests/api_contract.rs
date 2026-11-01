//! The public API as an external crate sees it (integration tests are one):
//! every option type is built from `Default`, by public-field mutation or
//! `with_*` setters, and every data type is read but never constructed by
//! struct literal, since all are `#[non_exhaustive]`. Each setter's effect
//! is observed, so a setter that silently did nothing fails here.

use mae_org::{
    parse, render, semantics_with, ArchivedTrees, DiagnosticKind, HeadlineIds, LinkTypes, Node,
    ParseOptions, RenderOptions, Resolved, Resolver, SemanticsOptions, TodoSequence, API_VERSION,
};

fn html(nodes: &[Node]) -> String {
    mae_org::to_html(nodes)
}

#[test]
fn parse_options_build_by_setter_and_by_field() {
    let deep = "- a\n  - b\n    - c\n      - d\n";
    let by_setter = ParseOptions::default().with_max_depth(2);
    let mut by_field = ParseOptions::default();
    by_field.max_depth = 2;
    assert_eq!(by_setter, by_field);
    let doc = parse(deep, &by_setter);
    assert!(doc
        .diagnostics()
        .iter()
        .any(|d| d.kind == DiagnosticKind::DepthLimit));

    let todo = ParseOptions::default()
        .with_todo_sequences(vec![TodoSequence::new(["NEXT"], ["SHIPPED"])])
        .with_link_types(["https"]);
    let doc = parse("* NEXT a\n* SHIPPED b\n", &todo);
    let kws: Vec<_> = doc.headlines().map(|h| h.todo()).collect();
    assert_eq!(kws, vec![Some("NEXT".into()), Some("SHIPPED".into())]);
    assert_eq!(todo.link_types, vec!["https".to_string()]);
}

#[test]
fn semantics_and_render_options_and_resolvers_build_from_outside() {
    let text = "#+title: File\n* Shown\n:PROPERTIES:\n:ID: n\n:END:\nSee [[id:m][*bold* link]].\n* Gone :private:\n* Old :OLD:\n:PROPERTIES:\n:ID: old\n:END:\nold body\n";
    let opts = SemanticsOptions::default()
        .with_exclude_tags(["private"])
        .with_select_tags(Vec::<String>::new())
        .with_archive_tag("OLD")
        .with_archived_trees(ArchivedTrees::All)
        .with_link_types(
            LinkTypes::default()
                .with_web(["http", "https"])
                .with_mail(["mailto"])
                .with_other(["file"]),
        )
        .with_link_exclude_keywords(["TRANSCLUDE"])
        .with_link_exclude_properties(["ROAM_REFS"]);
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics_with(&doc, &opts);
    let ids: Vec<&str> = sem.nodes.iter().map(|n| n.id.as_str()).collect();
    // `private` excluded; `OLD` is the archive tag and archived trees are
    // rendered whole, so `old` is a node.
    assert_eq!(ids, vec!["n", "old"]);

    let resolver = Resolver::default()
        .with_placeholder("(missing)")
        .with_target("m", Resolved::new("/m.html").with_title("M"));
    let r = render(
        &doc,
        &sem,
        &RenderOptions::default()
            .with_headline_ids(HeadlineIds::Slug)
            .with_images(true)
            .with_property_drawers(true),
        &resolver,
    );
    let body = html(&r.body);
    assert!(body.contains("href=\"/m.html\""), "{body}");
    assert!(body.contains("<h1 id=\"h-shown\">"), "{body}");
    assert!(!body.contains("id=\"id-n\""), "slug ids: {body}");
    assert!(body.contains("org-properties"), "{body}");
    assert!(body.contains("old body"), "{body}");
    assert!(!body.contains("Gone"), "{body}");
    assert_eq!(API_VERSION, "0.1.0");
}

#[test]
fn an_unlinked_title_holds_no_anchor() {
    let text = "* See [[https://x.example][*a* b]] and [[id:m]] and [[id:gone]]\n:PROPERTIES:\n:ID: t\n:END:\n";
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics_with(&doc, &SemanticsOptions::default());
    let resolver = Resolver::default().with_target("m", Resolved::new("/m.html").with_title("Em"));
    let r = render(&doc, &sem, &RenderOptions::default(), &resolver);
    let node = &r.nodes[0];
    let linked = html(&node.title);
    assert!(linked.contains("<a "), "{linked}");
    let unlinked = html(&node.title_unlinked);
    assert!(!unlinked.contains("<a"), "{unlinked}");
    // What the links show stays: descriptions, the resolved title, and the
    // placeholder of an unresolved one.
    assert_eq!(
        mae_org::text_of(&node.title_unlinked),
        "See a b and Em and [?]",
        "{unlinked}"
    );
    assert!(unlinked.contains("<strong>a</strong>"), "{unlinked}");
}
