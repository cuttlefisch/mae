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
    // Every #+title, joined (org-get-title); link → description; markup
    // dropped; blanks collapsed.
    assert_eq!(
        file.title.as_deref(),
        Some("First title A linked and bold title")
    );
    // #+kind replaces :KIND:, #+aliases are comma-separated.
    assert_eq!(file.kind.as_deref(), Some("lesson"));
    assert_eq!(file.aliases, vec!["one", "two"]);
    let h = &s.nodes[1];
    assert_eq!(
        h.title.as_deref(),
        Some("A styled headline with code and a link")
    );
    assert_eq!((h.todo.as_deref(), h.priority), (Some("TODO"), Some('A')));
    assert_eq!(h.own_tags, vec!["tag"]);
    // A link without a description reads as its path, as Org displays it.
    assert_eq!(s.nodes[2].title.as_deref(), Some("id:sem-title-bare only"));
    // Every other object stays exactly as written.
    assert_eq!(
        s.nodes[3].title.as_deref(),
        Some("Objects [1/2] \\alpha <2026-10-31 Sat> {{{macro}}} stay as written")
    );

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

/// org-roam's `org-roam-db-extra-links-elements` default, `(node-property
/// keyword)`: a link in a keyword or property value is an edge of the node
/// that owns the value, except the excluded keys (`ROAM_REFS`, `TRANSCLUDE`)
/// and raw export keywords and snippets, which are output text, not Org.
#[test]
fn keyword_and_property_values_are_link_sources() {
    let s = sem(&fixture("titles.org"));
    let title = s.links.iter().find(|l| l.target == "sem-title-target");
    assert_eq!(
        title.map(|l| (l.from.as_deref(), l.description.as_deref())),
        Some((Some("sem-title-file"), Some("linked")))
    );
    let text = ":PROPERTIES:\n:ID: f\n:URL: [[id:in-a-property]] and [[id:second]]\n:ROAM_REFS: [[id:a-ref]]\n:END:\n#+subtitle: [[id:in-a-keyword]]\n#+TRANSCLUDE: [[id:transcluded]]\n#+HTML: <a>[[id:raw-html]]</a>\nText @@html:[[id:in-a-snippet]]@@.\n#+caption: [[id:in-a-caption]]\n| x |\n* H\n:PROPERTIES:\n:ID: h\n:Source: [[id:from-h]]\n:END:\n#+keyword_in_h: [[id:kw-h]]\n- item\n  :PROPERTIES:\n  :ID: i\n  :SEE: [[id:from-item]]\n  :roam_refs: [[id:item-ref]]\n  :END:\n";
    let s = sem(text);
    let want = [
        ("f", "in-a-property"),
        ("f", "second"),
        ("f", "in-a-keyword"),
        ("f", "in-a-caption"),
        ("h", "from-h"),
        ("h", "kw-h"),
        ("i", "from-item"),
    ];
    assert_eq!(edges(&s), pairs(&want));
    // Each value link's span is the link itself in the document.
    for l in &s.links {
        assert_eq!(&text[l.span.range()], format!("[[{}]]", l.path), "{l:?}");
    }
    // Excluded keys are not even diagnosed when unrendered.
    let hidden = sem("* x :noexport:\n:PROPERTIES:\n:URL: [[id:u]]\n:ROAM_REFS: [[id:r]]\n:END:\n#+title: [[id:t]]\n#+HTML: [[id:h]]\n");
    let got: Vec<String> = unrendered_links(&hidden).into_iter().map(|l| l.0).collect();
    assert_eq!(got, vec!["id:u", "id:t"]);
}

#[test]
fn an_id_and_its_property_agree_on_what_is_trimmed() {
    // Org trims space, tab, LF and CR only: a no-break space is content.
    let s = sem(":PROPERTIES:\n:ID: x\u{a0}\n:ALIASES: a\u{a0}, \u{a0}b\n:END:\n");
    let n = &s.nodes[0];
    assert_eq!(n.id, "x\u{a0}");
    assert_eq!(n.properties[0], ("ID".to_string(), "x\u{a0}".to_string()));
    assert_eq!(n.aliases, vec!["a\u{a0}", "\u{a0}b"]);
    let s = sem("* h\n:PROPERTIES:\n:ID: \u{a0}y\n:END:\n");
    assert_eq!(s.nodes[0].id, "\u{a0}y");
}

/// One rule for every file keyword: document order, the last wins, an empty
/// last value clears (Org's reading of repeated keywords). The file's
/// property drawer precedes its keywords, so a keyword wins over `:KIND:` and
/// `:ALIASES:`; a keyword before the drawer makes the drawer a plain drawer.
#[test]
fn file_keywords_last_wins_and_an_empty_value_clears() {
    let file = |body: &str| {
        let s = sem(&format!(
            ":PROPERTIES:\n:ID: f\n:KIND: concept\n:ALIASES: p1, p2\n:END:\n{body}"
        ));
        s.nodes[0].clone()
    };
    let n = file("#+todo_state: TODO\n#+priority: B\n#+kind: lesson\n#+aliases: k1, k2\n");
    assert_eq!((n.todo.as_deref(), n.priority), (Some("TODO"), Some('B')));
    assert_eq!(
        (n.kind.as_deref(), n.aliases.clone()),
        (Some("lesson"), vec!["k1".to_string(), "k2".to_string()])
    );
    let n = file("#+todo_state: TODO\n#+todo_state:\n#+priority: B\n#+priority:\n#+kind: lesson\n#+kind:\n#+aliases: k1\n#+aliases:\n#+title: T\n#+title:\n");
    // MAE's keywords clear; the title is Org's (`org-get-title`): an empty
    // `#+title:` adds nothing and clears nothing.
    assert_eq!(
        (n.todo, n.priority, n.kind, n.title.as_deref()),
        (None, None, None, Some("T"))
    );
    assert!(n.aliases.is_empty(), "{:?}", n.aliases);
    let n = file("#+todo_state: WAIT\n#+todo_state: NEXT\n");
    assert_eq!(n.todo.as_deref(), Some("NEXT"));
    // Without keywords the drawer's own KIND and ALIASES hold.
    let n = file("");
    assert_eq!(
        (n.kind.as_deref(), n.aliases.clone()),
        (Some("concept"), vec!["p1".to_string(), "p2".to_string()])
    );
    // A keyword first: the drawer is not the file's, so no file node.
    let s = sem("#+aliases: k\n:PROPERTIES:\n:ID: f\n:ALIASES: p\n:END:\n");
    assert!(s.nodes.is_empty(), "{:?}", s.nodes);
}

/// The fixtures the differential compares with mae-kb, which kept the
/// first non-empty `#+todo_state` and read a drawer after a keyword.
#[test]
fn keyword_precedence_fixtures() {
    let s = sem(&fixture("keyword-precedence.org"));
    let n = &s.nodes[0];
    assert_eq!((n.todo.as_deref(), n.priority), (None, Some('A')));
    assert_eq!(
        (n.kind.as_deref(), n.aliases.clone()),
        (Some("lesson"), vec!["from-keyword-2".to_string()])
    );
    let s = sem(&fixture("keyword-before-drawer.org"));
    assert!(s.nodes.is_empty(), "{:?}", s.nodes);
    assert_eq!(
        unrendered_ids(&s),
        vec![("sem-kw-late-drawer".to_string(), Region::Drawer)]
    );
}

#[test]
fn every_node_gets_the_file_tags_once() {
    let s = sem("#+filetags: :ft:\n* H :own:\n:PROPERTIES:\n:ID: h\n:END:\n- item\n  :PROPERTIES:\n  :ID: i\n  :END:\n");
    assert_eq!(s.tags_of(&s.nodes[0]), vec!["ft", "own"]);
    assert_eq!(s.tags_of(&s.nodes[1]), vec!["ft"]);
    // Repeats in a headline's tags appear once.
    let s = sem("* H :a:b:a::b:\n:PROPERTIES:\n:ID: h\n:END:\n");
    assert_eq!(s.nodes[0].own_tags, vec!["a", "b"]);
}

/// org-transclusion's form: the member is the link's target, and its
/// properties are kept as args. MAE's own form is unchanged.
#[test]
fn org_transclusion_links_give_their_target() {
    let text = ":PROPERTIES:\n:ID: f\n:END:\n#+TRANSCLUDE: [[id:x][X]] :level 2 :only-contents\n#+transclude: [[file:notes.org::*h]]\n#+TRANSCLUDE: [[id:y]]\n#+TRANSCLUDE: plain-id reference\n#+TRANSCLUDE: lone\t\n";
    let s = sem(text);
    let got: Vec<(&str, &str, Option<&str>)> = s
        .transclusions
        .iter()
        .map(|t| (t.member.as_str(), t.role.as_str(), t.args.as_deref()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("x", "content", Some(":level 2 :only-contents")),
            ("file:notes.org", "content", None),
            ("y", "content", None),
            ("plain-id", "reference", None),
            ("lone", "content", None),
        ]
    );
    // The span is the keyword line without its line ending or blanks.
    let spans: Vec<&str> = s
        .transclusions
        .iter()
        .map(|t| &text[t.span.range()])
        .collect();
    assert_eq!(spans[4], "#+TRANSCLUDE: lone");
    assert_eq!(
        spans[0],
        "#+TRANSCLUDE: [[id:x][X]] :level 2 :only-contents"
    );
}

/// `drawer-after-blank.org`: a property drawer after a blank line is not the
/// headline's (Org reads it only right under the headline or planning
/// line), so it is a plain drawer, unrendered, and its id is no node.
#[test]
fn a_property_drawer_after_a_blank_line_is_a_plain_drawer() {
    let s = sem(&fixture("drawer-after-blank.org"));
    assert_eq!(ids(&s), vec!["sem-blank-file"]);
    assert_eq!(
        unrendered_ids(&s),
        vec![("sem-blank-heading".to_string(), Region::Drawer)]
    );
    assert_eq!(edges(&s), pairs(&[("sem-blank-file", "sem-blank-target")]));
}

/// `external-links.org`: every link kind, in order, as `classify_link`
/// reads it inside a document.
#[test]
fn external_links_fixture_classifies_each_kind() {
    let s = sem(&fixture("external-links.org"));
    let got: Vec<(LinkKind, &str, Option<&str>)> = s
        .links
        .iter()
        .map(|l| (l.kind, l.target.as_str(), l.search.as_deref()))
        .collect();
    use LinkKind::*;
    let want = vec![
        (Web, "https://example.invalid/a?b=c#d", None),
        (Web, "HTTP://EXAMPLE.INVALID", None),
        (Mailto, "mailto:someone@example.invalid", None),
        (Other, "file:notes.org", None),
        (Other, "doi:10.1000/1", None),
        (Other, "help:org-mode", None),
        (Other, "file+sys:/bin/sh", None),
        (Kb, "ssh://host.invalid", None),
        (Kb, "0f1e2d3c-0000-4000-8000-00000000abcd", None),
        (Kb, "concept:buffer", None),
        (CustomId, "custom-id", None),
        (Heading, "A heading", None),
        (Coderef, "ref", None),
        (Kb, "", None),
        (Other, "roam:Some title", None),
        (Other, "cite:key2026", None),
        (Other, "zotero://select/items/X", None),
        (Kb, "sem-external-search", Some("*A heading")),
        (Other, "file:notes.org", Some("*h")),
        (Kb, "sem-external-plain", None),
    ];
    assert_eq!(got, want);
}

/// (target, fragment, rel, weight, confidence, description).
type SplitRead<'a> = (&'a str, Option<&'a str>, &'a str, f64, f64, Option<&'a str>);

/// `split-orders.org`: each link where the old splitters disagreed, read
/// by the chosen order (`tests/link_grammar.rs` pins the order itself).
#[test]
fn split_orders_fixture_reads_each_link() {
    let s = sem(&fixture("split-orders.org"));
    let got: Vec<SplitRead> = s
        .links
        .iter()
        .map(|l| {
            let d = l.description.as_deref();
            (
                l.target.as_str(),
                l.fragment.as_deref(),
                l.rel.as_str(),
                l.weight,
                l.confidence,
                d,
            )
        })
        .collect();
    let want = vec![
        ("sem-split-a", None, "references", 1.0, 1.0, Some("c")),
        ("sem-split-q", None, "cites#frag", 1.0, 1.0, None),
        ("sem-split-f", Some("frag"), "cites", 0.5, 1.0, None),
        ("sem-split-m", None, "references", 1.0, 1.0, None),
        ("sem-split-stray", None, "references", 1.0, 1.0, None),
        ("sem-split-one", None, "references", 1.0, 1.0, None),
        ("sem-split-two", None, "references", 1.0, 1.0, Some("two")),
        (
            "sem-split-s",
            None,
            "references",
            1.0,
            1.0,
            Some("What? #1"),
        ),
    ];
    assert_eq!(got, want);
    let attrs: Vec<&[(String, String)]> = s.links.iter().map(|l| l.attrs.as_slice()).collect();
    assert_eq!(attrs[2], &[("by".to_string(), "ai".to_string())]);
}

/// Every fixture under `tests/semantics/` is named by some test, so none is
/// read by the differential alone with nothing pinning what it means.
#[test]
fn every_semantics_fixture_is_named_by_a_test() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut sources = String::new();
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        let differential = p
            .file_name()
            .is_some_and(|f| f == "semantics_differential.rs");
        if p.extension().is_some_and(|x| x == "rs") && !differential {
            sources.push_str(&support::read(&p));
        }
    }
    let fixtures = support::org_files(&dir.join("semantics"));
    assert!(!fixtures.is_empty());
    for f in fixtures {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            sources.contains(&format!("fixture(\"{name}\")")),
            "no test reads tests/semantics/{name}"
        );
    }
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
    assert_eq!(s.file_tags, vec!["alpha", "beta", "gamma", "delta"]);
    assert_eq!(s.tags_of(&s.nodes[0]), s.file_tags);
    assert!(s.nodes[0].own_tags.is_empty());
    // `beta` is a file tag already: the heading's own tags are what is left.
    assert_eq!(s.nodes[1].own_tags, vec!["own"]);
    assert_eq!(
        s.tags_of(&s.nodes[1]),
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
    // An item's keys are canonical like a drawer's.
    assert_eq!(cb.properties[2].0, "ITEM-KEY");
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
        ..SemanticsOptions::default()
    };
    let s = semantics_with(&parse(plain, &ParseOptions::default()), &opts);
    assert_eq!(ids(&s), vec!["b"]);
    assert_eq!(ids(&sem(plain)), vec!["a"]);
}

#[test]
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

/// Registered (semantics-divergences.toml, `plain_link`): Org registers `id`
/// as a link type, so `id:x` in prose is an id link — a KB edge — though
/// mae-kb only ever read bracket links. Other text with a colon is not.
#[test]
fn plain_id_links_are_links() {
    let doc = mae_org::parse(
        ":PROPERTIES:\n:ID: n\n:END:\nSee id:target and concept:not-a-type here.\n",
        &mae_org::ParseOptions::default(),
    );
    let targets: Vec<String> = mae_org::semantics(&doc)
        .links
        .iter()
        .map(|l| l.target.clone())
        .collect();
    assert_eq!(targets, vec!["target".to_string()]);
}

/// A plain link's path may end in a content CR in an LF file (Org's
/// `org-link-plain-re` takes it; the tree pins that in tests/inline.rs).
/// The semantic link must cover what the tree covers: its span holds its
/// path, in a headline title and in a paragraph alike.
#[test]
fn a_plain_link_ending_in_a_content_cr_keeps_it_in_its_span() {
    let text = "* Head http:x\r\n:PROPERTIES:\n:ID: h\n:END:\nSee https://a.b/c\r\nmore\n";
    let s = semantics(&parse(text, &ParseOptions::default()));
    let links: Vec<(&str, &str)> = s
        .links
        .iter()
        .map(|l| (l.path.as_str(), &text[l.span.range()]))
        .collect();
    assert_eq!(
        links,
        vec![
            ("http:x\r", "http:x\r"),
            ("https://a.b/c\r", "https://a.b/c\r")
        ]
    );
}

/// Review F9. A file's title is `org-get-title`'s (Emacs 30.2): every line
/// starting `#+TITLE:` — in any context: a COMMENT subtree, a src block —
/// the values joined with a space and trimmed, read as plain text. Emacs on
/// this text: "First Second  InComment InBlock" (an empty value adds its
/// space, a value never clears the title).
#[test]
fn the_file_title_is_org_get_title() {
    let text = ":PROPERTIES:\n:ID: f\n:END:\n#+title: First\n#+title: Second\n#+title:\n\n* COMMENT c\n#+title: InComment\n* b\n#+begin_src text\n#+title: InBlock\n#+end_src\n";
    assert_eq!(
        sem(text).nodes[0].title.as_deref(),
        Some("First Second InComment InBlock")
    );
    let title = |t: &str| {
        sem(&format!(":PROPERTIES:\n:ID: f\n:END:\n{t}")).nodes[0]
            .title
            .clone()
    };
    // Markup spanning two values reads as one: Org joins before parsing.
    assert_eq!(title("#+title: *a\n#+title: b*\n").as_deref(), Some("a b"));
    // Only empty values: no title; a key runs to the last colon of its word.
    assert_eq!(title("#+title:\n#+TITLE:  \n"), None);
    assert_eq!(
        title("  #+Title: x:y\n#+title:z:w\n").as_deref(),
        Some("x:y w")
    );
    // `#+titles:` is another keyword.
    assert_eq!(title("#+titles: no\n"), None);
}

/// MAE's own keywords (`#+kind`, `#+aliases`, `#+todo_state`,
/// `#+priority`) have no Emacs reading: their rule is MAE's — the zeroth
/// section, document order, the last wins, an empty value clears.
#[test]
fn maes_own_file_keywords_keep_maes_rule() {
    let s = sem(
        ":PROPERTIES:\n:ID: f\n:END:\n#+todo_state: TODO\n#+todo_state:\n#+kind: a\n#+kind: b\n",
    );
    assert_eq!(
        (s.nodes[0].todo.as_deref(), s.nodes[0].kind.as_deref()),
        (None, Some("b"))
    );
}

/// Review F9: `KEY+` as `org-entry-get` reads it: the base is the last
/// `KEY` line wherever it is, followed by every `KEY+` value in order (so a
/// `KEY+` before `KEY` still appends), and a value that is exactly `nil` is
/// no value. The same rule for an item's drawer, and so for its `:ID:`.
#[test]
fn key_plus_reads_as_org_entry_get() {
    let s = sem("* h\n:PROPERTIES:\n:K+: b\n:K: a\n:K+: c\n:K: z\n:ID: h\n:ID+: more\n:N: nil\n:N+: kept\n:END:\n- item\n  :PROPERTIES:\n  :ID+: two\n  :ID: one\n  :END:\n");
    let props = &s.nodes[0].properties;
    let get = |k: &str| props.iter().find(|p| p.0 == k).map(|p| p.1.as_str());
    assert_eq!(get("K"), Some("z b c"));
    assert_eq!(get("N"), Some("kept"));
    assert_eq!(s.nodes[0].id, "h more");
    assert_eq!(s.nodes[1].id, "one two");
    // The fixture the Emacs oracle reads too (semantics-titles.tsv, through
    // `org-entry-get`): file, headline and item ids all appended.
    assert_eq!(
        ids(&sem(&fixture("id-append.org"))),
        vec![
            "sem-idplus-file part",
            "sem-idplus-heading tail",
            "sem-idplus-item more"
        ]
    );
}
