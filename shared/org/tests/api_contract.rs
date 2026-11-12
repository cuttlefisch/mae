//! The public API as an external crate sees it (integration tests are one):
//! every option type is built from `Default`, by public-field mutation or
//! `with_*` setters, and every data type is read but never constructed by
//! struct literal, since all are `#[non_exhaustive]`. Each setter's effect
//! is observed, so a setter that silently did nothing fails here.

use mae_org::{
    parse, parse_fragment, render, semantics_with, ArchivedTrees, DiagnosticKind, FragmentContext,
    HeadlineIds, LineEnding, LinkFormat, LinkTypes, Node, ParseOptions, RenderOptions, Resolved,
    Resolver, SemanticsOptions, TodoSequence, API_VERSION,
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
    assert_eq!(API_VERSION, "0.5.1");
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

#[test]
fn a_fragment_context_builds_from_a_document_or_by_setter() {
    let text = "#+TODO: WAIT | GONE\n<<<Widget>>> here\n* H\nWAIT a Widget\n";
    let doc = parse(text, &ParseOptions::default());
    let ctx = FragmentContext::from_document(&doc, doc.headlines().next().as_ref());
    assert_eq!(ctx.level, 1);
    assert_eq!(ctx.ancestors, 1);
    assert_eq!(ctx.line_ending, LineEnding::Lf);
    assert_eq!(
        ctx.radio_targets.as_deref(),
        Some(&["Widget".to_string()][..])
    );
    let by_setter = FragmentContext::new(9, LineEnding::CrLf)
        .with_level(1)
        .with_ancestors(1)
        .with_line_ending(LineEnding::Lf)
        .with_radio_targets(["Widget"])
        .with_todo_sequences(vec![TodoSequence::new(["WAIT"], ["GONE"])]);
    assert_eq!(by_setter, ctx);
    let mut by_field = FragmentContext::new(1, LineEnding::Lf);
    by_field.radio_targets = Some(vec!["Widget".into()]);
    by_field.todo_sequences = Some(doc.todo_sequences().to_vec());
    assert_eq!(by_field, ctx);
    let frag = parse_fragment("WAIT a Widget\n", &ParseOptions::default(), &ctx);
    assert_eq!(frag.radio_targets(), ["Widget".to_string()]);
    assert_eq!(frag.todo_sequences(), doc.todo_sequences());
}

#[test]
fn views_are_read_from_outside() {
    let doc = parse(
        "* H\n:PROPERTIES:\n:Key+: v\n:END:\n[[https://x.example/a][d]]\n",
        &ParseOptions::default(),
    );
    let h = doc.headlines().next().unwrap();
    let p = &h.property_drawer().unwrap().properties()[0];
    assert_eq!((p.key.as_str(), p.value.as_str()), ("KEY+", "v"));
    assert_eq!(p.key_as_written(), "Key+");
    assert!(p.is_append());
    let link = doc
        .syntax()
        .descendants()
        .find_map(mae_org::Link::cast)
        .unwrap();
    let info = link.info(&["https"]);
    assert_eq!(info.link_type, "https");
    assert_eq!(info.path, "//x.example/a");
    assert_eq!(info.format, LinkFormat::Bracket);
    assert!(info.type_explicit && info.search_option.is_none() && info.application.is_none());
    assert_eq!(info.raw_link, "https://x.example/a");
}
