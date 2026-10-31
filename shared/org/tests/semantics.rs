//! `semantics()`: KB nodes, links and transclusions from rendered regions
//! only (ADR-112 D2). Each test pins one rule with a selective oracle; the
//! fixtures under `tests/semantics/` are shared with the mae-kb differential
//! (`semantics_differential.rs`), whose register names these tests.

mod support;

use mae_org::{
    parse, semantics, semantics_with, LinkKind, NodeOrigin, ParseOptions, Region,
    SemanticDiagnostic, Semantics, SemanticsOptions, TodoSequence,
};
use std::path::Path;

fn sem(text: &str) -> Semantics {
    semantics(&parse(text, &ParseOptions::default()))
}

fn fixture(name: &str) -> String {
    support::read(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/semantics")
            .join(name),
    )
}

fn ids(s: &Semantics) -> Vec<&str> {
    s.nodes.iter().map(|n| n.id.as_str()).collect()
}

/// (from, target) of every KB link.
fn edges(s: &Semantics) -> Vec<(String, String)> {
    s.links
        .iter()
        .filter(|l| l.kind == LinkKind::Kb)
        .map(|l| (l.from.clone().unwrap_or_default(), l.target.clone()))
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn unrendered_ids(s: &Semantics) -> Vec<(String, Region)> {
    s.diagnostics
        .iter()
        .filter_map(|d| match d {
            SemanticDiagnostic::IdInUnrenderedRegion { id, region, .. } => {
                Some((id.clone(), *region))
            }
            _ => None,
        })
        .collect()
}

fn unrendered_links(s: &Semantics) -> Vec<(String, Region)> {
    s.diagnostics
        .iter()
        .filter_map(|d| match d {
            SemanticDiagnostic::LinkInUnrenderedRegion { path, region, .. } => {
                Some((path.clone(), *region))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn titles_are_plain_text() {
    let s = sem(&fixture("titles.org"));
    let file = &s.nodes[0];
    // The last #+title wins; link → description; markup dropped; blanks collapsed.
    assert_eq!(file.title.as_deref(), Some("A linked and bold title"));
    // #+kind replaces :KIND:, #+aliases are comma-separated.
    assert_eq!(file.kind.as_deref(), Some("lesson"));
    assert_eq!(file.aliases, vec!["one", "two"]);
    let h = &s.nodes[1];
    assert_eq!(
        h.title.as_deref(),
        Some("A styled headline with code and a link")
    );
    assert_eq!((h.todo.as_deref(), h.priority), (Some("TODO"), Some('A')));
    assert_eq!(h.tags, vec!["tag"]);
    // A link without a description reads as its path, as Org displays it.
    assert_eq!(s.nodes[2].title.as_deref(), Some("id:sem-title-bare only"));

    let t = |title: &str| {
        let s = sem(&format!("* {title}\n:PROPERTIES:\n:ID: x\n:END:\n"));
        s.nodes[0].title.clone()
    };
    assert_eq!(
        t("*/nested/* =v= ~c~ +s+ _u_").as_deref(),
        Some("nested v c s u")
    );
    assert_eq!(
        t("[[x|Shown]] store form").as_deref(),
        Some("Shown store form")
    );
    assert_eq!(
        t("[[https://a.invalid]]").as_deref(),
        Some("https://a.invalid")
    );
    // Other objects stay as written: cookies, entities, timestamps.
    assert_eq!(
        t("Done [1/2] \\alpha <2026-10-31 Sat>").as_deref(),
        Some("Done [1/2] \\alpha <2026-10-31 Sat>")
    );
    assert_eq!(t("a \t  b"), Some("a b".to_string()));
    assert_eq!(t(""), None);
    assert_eq!(
        sem(":PROPERTIES:\n:ID: f\n:END:\n#+title:\n").nodes[0].title,
        None
    );
}

#[test]
fn property_keys_are_canonical_and_repeats_resolve_like_org() {
    let s = sem(&fixture("properties.org"));
    let file = &s.nodes[0];
    let want = [
        ("ID", "sem-props-file"),
        ("LAST-ACCESSED", "2026-10-31"),
        ("KIND", "concept"),
        ("ALIASES", "first, second ,, third"),
    ];
    assert_eq!(file.properties, pairs(&want));
    assert_eq!(file.kind.as_deref(), Some("concept"));
    assert_eq!(file.aliases, vec!["first", "second", "third"]);
    let h = &s.nodes[1];
    let want = [
        ("ID", "sem-props-heading"),
        // Keys fold case; the last value wins in the first one's place.
        ("CUSTOM_KEY", "last wins"),
        // KEY+ appends with a space.
        ("MULTI", "one two"),
        // An empty value is kept, not dropped.
        ("EMPTY", ""),
        ("KIND", "task"),
        ("ALIASES", "alias-a, alias-b"),
    ];
    assert_eq!(h.properties, pairs(&want));
    assert_eq!(h.kind.as_deref(), Some("task"));
    assert_eq!(h.aliases, vec!["alias-a", "alias-b"]);
}

#[test]
fn unrendered_regions_yield_only_diagnostics() {
    let s = sem(&fixture("unrendered.org"));
    assert_eq!(ids(&s), vec!["sem-unr-file", "sem-unr-visible-heading"]);
    assert_eq!(
        edges(&s),
        pairs(&[("sem-unr-file", "sem-unr-visible")]),
        "no link from any unrendered or verbatim region"
    );
    let want_ids = vec![
        ("sem-unr-commented-drawer".to_string(), Region::CommentLine),
        (
            "sem-unr-comment-heading".to_string(),
            Region::CommentSubtree,
        ),
        (
            "sem-unr-noexport-heading".to_string(),
            Region::ExcludedSubtree,
        ),
        (
            "sem-unr-noexport-child".to_string(),
            Region::ExcludedSubtree,
        ),
        ("sem-unr-in-comment-block".to_string(), Region::CommentBlock),
    ];
    assert_eq!(unrendered_ids(&s), want_ids);
    let want_links: Vec<(String, Region)> = [
        ("id:sem-unr-comment-line-link", Region::CommentLine),
        ("id:sem-unr-export-link", Region::ExportBlock),
        ("id:sem-unr-logbook-link", Region::Drawer),
        ("id:sem-unr-comment-heading-link", Region::CommentSubtree),
        ("id:sem-unr-noexport-link", Region::ExcludedSubtree),
        ("id:sem-unr-comment-block-link", Region::CommentBlock),
        ("id:sem-unr-footnote-link", Region::UnreferencedFootnote),
    ]
    .iter()
    .map(|(p, r)| (p.to_string(), *r))
    .collect();
    assert_eq!(unrendered_links(&s), want_links);
}

#[test]
fn verbatim_text_holds_no_links_or_directives() {
    let s = sem(&fixture("unrendered.org"));
    let text = format!("{:?}", s);
    for word in [
        "example-link",
        "src-link",
        "verbatim-link",
        "code-link",
        "fixed-width-link",
    ] {
        assert!(
            !text.contains(word),
            "{word} must be neither a link nor a diagnostic"
        );
    }
    let t = sem(&fixture("transclusions.org"));
    assert!(t
        .transclusions
        .iter()
        .all(|x| x.member != "sem-tr-in-example"));
}

#[test]
#[ignore = "needs the table-cell object parser (O2 inline objects.rs)"]
fn links_in_table_cells_are_links() {
    let s = sem(":PROPERTIES:\n:ID: f\n:END:\n| [[id:in-a-cell]] | x |\n");
    assert_eq!(edges(&s), pairs(&[("f", "in-a-cell")]));
}

#[test]
fn a_link_belongs_to_the_innermost_node_only() {
    let s = sem(&fixture("parent-child-links.org"));
    let want = [
        ("sem-pc-file", "sem-pc-a"),
        ("sem-pc-file", "sem-pc-shared"),
        ("sem-pc-parent", "sem-pc-b"),
        ("sem-pc-child", "sem-pc-c"),
        ("sem-pc-child", "sem-pc-shared"),
        // A headline without an id is part of the node around it.
        ("sem-pc-parent", "sem-pc-d"),
        ("sem-pc-file", "sem-pc-e"),
    ];
    assert_eq!(edges(&s), pairs(&want));
    let t = sem(&fixture("transclusions.org"));
    let d = t
        .transclusions
        .iter()
        .find(|x| x.member == "sem-tr-member-d")
        .unwrap();
    assert_eq!(d.from.as_deref(), Some("sem-tr-heading"));
    // Outside every node a link has no source, but it is still a link.
    let s = sem("Text [[id:x]].\n");
    assert_eq!(
        (s.links[0].from.as_deref(), s.links[0].target.as_str()),
        (None, "x")
    );
}

#[test]
fn duplicate_ids_are_diagnosed_and_not_nodes() {
    let text = fixture("duplicates.org");
    let s = sem(&text);
    assert_eq!(ids(&s), vec!["sem-dup-file", "sem-dup"]);
    assert_eq!(s.nodes[1].title.as_deref(), Some("First"));
    let dups: Vec<_> = s
        .diagnostics
        .iter()
        .filter_map(|d| match d {
            SemanticDiagnostic::DuplicateId { id, span, first } => Some((id, *span, *first)),
            _ => None,
        })
        .collect();
    assert_eq!(dups.len(), 1);
    let (id, span, first) = dups[0];
    assert_eq!(id, "sem-dup");
    assert_eq!(&text[span.range()], ":ID: sem-dup");
    assert!(text[span.range().start..].starts_with(":ID: sem-dup\n:END:\nSecond"));
    assert_eq!(first, s.nodes[1].span);
    // The second element's content belongs to the node around it.
    assert!(edges(&s).contains(&("sem-dup-file".into(), "sem-dup-second-link".into())));
    assert!(edges(&s).contains(&("sem-dup".into(), "sem-dup-first-link".into())));
}

#[test]
fn keyword_values_are_not_link_sources() {
    let s = sem(&fixture("titles.org"));
    assert!(s.links.iter().all(|l| l.target != "sem-title-target"));
    let s = sem(":PROPERTIES:\n:ID: f\n:URL: [[id:in-a-property]]\n:END:\n#+subtitle: [[id:in-a-keyword]]\n");
    assert!(s.links.is_empty(), "{:?}", s.links);
}

#[test]
fn todo_keywords_are_declared_data() {
    let s = sem(&fixture("todo-in-file.org"));
    let open = &s.nodes[1];
    assert_eq!(
        (open.todo.as_deref(), open.priority),
        (Some("OPEN"), Some('B'))
    );
    assert_eq!(
        open.title.as_deref(),
        Some("A task with an in-file keyword")
    );
    let next = &s.nodes[2];
    assert_eq!(
        (next.todo.as_deref(), next.title.as_deref()),
        (None, Some("NEXT A keyword the file does not declare"))
    );
    // Without an in-file declaration the configured sequences decide.
    let text = "* NEXT x\n:PROPERTIES:\n:ID: n\n:END:\n";
    assert_eq!(sem(text).nodes[0].todo, None);
    let opts = ParseOptions {
        todo_sequences: vec![TodoSequence {
            todo: vec!["NEXT".into()],
            done: vec!["DONE".into()],
        }],
        ..ParseOptions::default()
    };
    assert_eq!(
        semantics(&parse(text, &opts)).nodes[0].todo.as_deref(),
        Some("NEXT")
    );
}

#[test]
fn transclude_keywords_are_case_insensitive() {
    let s = sem(&fixture("transclusions.org"));
    let got: Vec<(Option<&str>, &str, &str)> = s
        .transclusions
        .iter()
        .map(|t| (t.from.as_deref(), t.member.as_str(), t.role.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (Some("sem-tr-file"), "sem-tr-member-a", "content"),
            (Some("sem-tr-file"), "sem-tr-member-b", "reference"),
            (Some("sem-tr-file"), "sem-tr-member-c", "transclusion"),
            (Some("sem-tr-heading"), "sem-tr-member-d", "content"),
        ]
    );
}

#[test]
fn filetags_split_like_org_and_tags_appear_once() {
    let s = sem(&fixture("file-tags.org"));
    // `#+tags:` declares the allowed tag set; it is not file tags.
    assert_eq!(s.nodes[0].tags, vec!["alpha", "beta", "gamma", "delta"]);
    assert_eq!(
        s.nodes[1].tags,
        vec!["alpha", "beta", "gamma", "delta", "own"]
    );
}

#[test]
fn indented_stars_are_never_headlines() {
    let text = fixture("indented-stars.org");
    let doc = parse(&text, &ParseOptions::default());
    assert_eq!(doc.headlines().count(), 0);
    let s = semantics(&doc);
    assert_eq!(ids(&s), vec!["sem-indented-file"]);
    // The drawers sit at the bullet's indentation, so outside the item: plain drawers.
    let want = vec![
        ("sem-indented-item".to_string(), Region::Drawer),
        ("sem-indented-text".to_string(), Region::Drawer),
    ];
    assert_eq!(unrendered_ids(&s), want);
}

#[test]
fn only_the_leading_property_drawer_is_the_files() {
    let s = sem(&fixture("zeroth-section-drawers.org"));
    assert_eq!(ids(&s), vec!["sem-zeroth-file", "sem-zeroth-item"]);
    assert_eq!(
        s.nodes[0].title.as_deref(),
        Some("A drawer later in the zeroth section")
    );
    let item = &s.nodes[1];
    assert_eq!((item.origin, item.level), (NodeOrigin::Item, 1));
    assert_eq!(item.title.as_deref(), Some("An item in the zeroth section"));
    assert_eq!(
        edges(&s),
        pairs(&[("sem-zeroth-item", "sem-zeroth-item-link")])
    );
}

#[test]
fn list_items_with_a_property_drawer_are_nodes() {
    let text = fixture("items.org");
    let s = sem(&text);
    assert_eq!(
        ids(&s),
        vec![
            "sem-items-file",
            "sem-items-heading",
            "sem-items-checkbox",
            "sem-items-descriptive"
        ]
    );
    let cb = &s.nodes[2];
    assert_eq!(
        (cb.origin, cb.level, cb.kind.as_deref()),
        (NodeOrigin::Item, 2, Some("task"))
    );
    assert_eq!(
        cb.title.as_deref(),
        Some("A checkbox item whose first line wraps onto a second")
    );
    assert_eq!(
        &text[cb.body_span.range()],
        "  Its body [[id:sem-items-checkbox-link]].\n"
    );
    assert_eq!(
        s.nodes[3].title.as_deref(),
        Some("term :: a descriptive item")
    );
    assert!(edges(&s).contains(&(
        "sem-items-checkbox".into(),
        "sem-items-checkbox-link".into()
    )));
    // Free text in the drawer, or a drawer after a paragraph: plain drawers.
    let want = vec![
        ("sem-items-not-props".to_string(), Region::Drawer),
        ("sem-items-paragraph-drawer".to_string(), Region::Drawer),
    ];
    assert_eq!(unrendered_ids(&s), want);
}

/// Each condition of the item-drawer extension on its own: the same item
/// is a node, then not, as one condition is broken; keys are canonical.
#[test]
fn item_property_drawers_need_their_place_name_and_syntax() {
    let item = |body: &str| sem(&format!("- First line\n{body}"));
    let ok = item("  :PROPERTIES:\n  :id: i\n  :kind: task\n  :END:\n");
    assert_eq!(ids(&ok), vec!["i"]);
    // Keys as written are folded to their canonical form.
    assert_eq!(
        ok.nodes[0].properties,
        pairs(&[("ID", "i"), ("KIND", "task")])
    );
    assert_eq!(ok.nodes[0].kind.as_deref(), Some("task"));
    // The drawer as the item's first element (no title line) still counts.
    assert_eq!(
        ids(&sem("- \n  :PROPERTIES:\n  :ID: i\n  :END:\n")),
        vec!["i"]
    );
    // Not the drawer's place: after a second paragraph.
    let late = item("\n  Second paragraph.\n  :PROPERTIES:\n  :ID: i\n  :END:\n");
    assert!(late.nodes.is_empty());
    assert_eq!(
        unrendered_ids(&late),
        vec![("i".to_string(), Region::Drawer)]
    );
    // Not its name: any other drawer, even holding property lines.
    let named = item("  :LOGBOOK:\n  :ID: i\n  :END:\n");
    assert!(named.nodes.is_empty());
    assert_eq!(
        unrendered_ids(&named),
        vec![("i".to_string(), Region::Drawer)]
    );
}

#[test]
fn body_spans_are_the_body_alone() {
    let text = "# -*- mode: org -*-\n:PROPERTIES:\n:ID: f\n:END:\n#+title: T\n\nFile body.\n\n* H\nSCHEDULED: <2026-10-31>\n:PROPERTIES:\n:ID: h\n:END:\nH body.\n\n** C\n:PROPERTIES:\n:ID: c\n:END:\n* E\n:PROPERTIES:\n:ID: e\n:END:\n";
    let s = sem(text);
    let body = |i: usize| &text[s.nodes[i].body_span.range()];
    assert_eq!(body(0), "File body.\n\n");
    // Up to the first child headline, trailing blank lines included.
    assert_eq!(body(1), "H body.\n\n");
    assert_eq!(body(2), "");
    assert_eq!(
        s.nodes[2].body_span.start as usize,
        text.find("* E").unwrap()
    );
    assert_eq!(body(3), "");
    assert_eq!(s.nodes[3].body_span.start as usize, text.len());
    // Spans: the file, the subtree.
    assert_eq!(&text[s.nodes[0].span.range()], text);
    assert_eq!(
        &text[s.nodes[1].span.range()],
        &text[text.find("* H").unwrap()..text.find("* E").unwrap()]
    );
    // A headline with no section: an empty body where the child starts.
    let t2 = "* P\n:PROPERTIES:\n:ID: p\n:END:\n";
    let t3 = "* P\n** Q\n";
    assert_eq!(&t2[sem(t2).nodes[0].body_span.range()], "");
    let doc = parse(t3, &ParseOptions::default());
    assert!(semantics(&doc).nodes.is_empty());
}

#[test]
fn exclude_tags_are_declared_data() {
    let text = ":PROPERTIES:\n:ID: f\n:END:\n#+EXCLUDE_TAGS: private\n* A :noexport:\n:PROPERTIES:\n:ID: a\n:END:\n* B :private:\n:PROPERTIES:\n:ID: b\n:END:\n";
    assert_eq!(ids(&sem(text)), vec!["f", "a"]);
    let plain =
        "* A :secret:\n:PROPERTIES:\n:ID: a\n:END:\n* B :noexport:\n:PROPERTIES:\n:ID: b\n:END:\n";
    let opts = SemanticsOptions {
        exclude_tags: vec!["secret".into()],
    };
    let s = semantics_with(&parse(plain, &ParseOptions::default()), &opts);
    assert_eq!(ids(&s), vec!["b"]);
    assert_eq!(ids(&sem(plain)), vec!["a"]);
}

#[test]
#[ignore = "needs the footnote-reference object parser (O2 inline objects.rs)"]
fn referenced_footnotes_are_rendered() {
    let text = ":PROPERTIES:\n:ID: f\n:END:\nSee[fn:1].\n\n[fn:1] Def [[id:x]] and[fn:2].\n\n[fn:2] Chained [[id:y]].\n\n[fn:3] Orphan [[id:z]].\n";
    let s = sem(text);
    assert_eq!(edges(&s), pairs(&[("f", "x"), ("f", "y")]));
    assert_eq!(
        unrendered_links(&s),
        vec![("id:z".to_string(), Region::UnreferencedFootnote)]
    );
}

#[test]
fn links_carry_kind_description_and_a_span_without_trailing_blanks() {
    let text = "[[https://a.invalid][A *site*]]   and [[mailto:x@y.invalid]] [[id:]] <https://b.invalid>\n";
    let s = sem(text);
    let l = &s.links;
    assert_eq!(l.len(), 4);
    assert_eq!(
        (l[0].kind, l[0].description.as_deref()),
        (LinkKind::Web, Some("A site"))
    );
    assert_eq!(&text[l[0].span.range()], "[[https://a.invalid][A *site*]]");
    assert_eq!(l[1].kind, LinkKind::Mailto);
    // An empty KB link is reported as written; whether it is an edge is the consumer's call.
    assert_eq!(
        (l[2].kind, l[2].target.as_str(), l[2].fragment.as_deref()),
        (LinkKind::Kb, "", None)
    );
    assert_eq!(
        (l[3].kind, l[3].path.as_str()),
        (LinkKind::Web, "https://b.invalid")
    );
}

/// Over every corpus file and fixture: deterministic, ordered, and every
/// span inside the text and on character boundaries, every body inside its
/// node, every link span starting a link.
#[test]
fn semantics_are_deterministic_and_well_formed_over_the_corpus() {
    let mut files = support::bundled_corpus();
    files.extend(support::org_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/semantics"),
    ));
    files.extend(support::fixtures());
    for path in &files {
        let text = support::read(path);
        let a = sem(&text);
        assert_eq!(a, sem(&text), "{}", path.display());
        let ok = |s: mae_org::Span| {
            s.start <= s.end
                && (s.end as usize) <= text.len()
                && text.is_char_boundary(s.start as usize)
                && text.is_char_boundary(s.end as usize)
        };
        for n in &a.nodes {
            assert!(
                ok(n.span) && ok(n.body_span),
                "{}: {}",
                path.display(),
                n.id
            );
            assert!(
                n.span.start <= n.body_span.start && n.body_span.end <= n.span.end,
                "{}: {}",
                path.display(),
                n.id
            );
        }
        let starts: Vec<u32> = a.nodes.iter().map(|n| n.span.start).collect();
        assert!(
            starts.windows(2).all(|w| w[0] <= w[1]),
            "{}",
            path.display()
        );
        for l in &a.links {
            assert!(ok(l.span), "{}", path.display());
            assert!(
                text[l.span.range()].contains(l.path.as_str()),
                "{}: {:?}",
                path.display(),
                l
            );
        }
        let keys: Vec<_> = a
            .diagnostics
            .iter()
            .map(|d| (d.span(), d.kind_name()))
            .collect();
        assert!(keys.windows(2).all(|w| w[0] <= w[1]), "{}", path.display());
    }
}

/// D2's closure, consumer note (a): an id that occurs only in unrendered
/// regions is never a node, whatever region hides it.
#[test]
fn an_id_only_in_an_unrendered_region_is_never_a_node() {
    let wrappers: [(&str, &str, Region); 6] = [
        ("#+begin_comment\n", "#+end_comment\n", Region::CommentBlock),
        (
            "#+begin_export org\n",
            "#+end_export\n",
            Region::ExportBlock,
        ),
        ("* COMMENT c\n", "", Region::CommentSubtree),
        ("* x :noexport:\n", "", Region::ExcludedSubtree),
        ("* x\n:LOGBOOK:\n", ":END:\n", Region::Drawer),
        ("[fn:n] A note\n", "", Region::UnreferencedFootnote),
    ];
    for (open, close, region) in wrappers {
        let inner = if open.starts_with("[fn") {
            ":ID: hidden\n"
        } else {
            ":PROPERTIES:\n:ID: hidden\n:END:\n"
        };
        let text = format!("{open}{inner}{close}");
        let s = sem(&text);
        assert!(s.nodes.iter().all(|n| n.id != "hidden"), "{text}");
        assert_eq!(
            unrendered_ids(&s),
            vec![("hidden".to_string(), region)],
            "{text}"
        );
    }
}
