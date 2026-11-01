//! Hidden inline constructs stay hidden in every container (ADR-112 D3,
//! review B1/B2): a marker placed inside every inline construct the export
//! removes (an inline src block its `:exports` hides, the results macro of
//! one, an inline babel call and its hidden results, an export snippet) and
//! that construct placed in every container that holds objects (link
//! descriptions of each kind, headings, item tags, table cells, captions,
//! footnote definitions and references, property values, aliases, the file
//! title) must reach no output: the hast, the HTML, `text()`, `node_text`,
//! the node metadata, and therefore no id either.
//!
//! Exhaustive (every construct in every container, under every option
//! set), not sampled: the product is small, and a missed cell is exactly
//! the leak this guards against.

use mae_org::{
    node_text, parse, render, semantics, text, HeadlineIds, ParseOptions, RenderOptions, Resolved,
    Resolver, TreeFormat,
};

/// Inline constructs whose `{M}` must never be shown. Each is a fragment of
/// one line.
const HIDDEN: &[(&str, &str)] = &[
    (
        "inline src, :exports none, and its results",
        "src_sh[:exports none]{echo {M}} {{{results(={M}=)}}}",
    ),
    ("inline src, default (results)", "src_sh{echo {M}}"),
    (
        "inline src, :exports results",
        "src_sh[:exports results]{echo {M}}",
    ),
    (
        "results of an inline src with :exports code",
        "src_sh[:exports code]{shown} {{{results(={M}=)}}}",
    ),
    (
        "inline src, :exports a number",
        "src_sh[:exports 1e3]{echo {M}} {{{results(={M}=)}}}",
    ),
    ("inline babel call arguments", "call_fn({M})"),
    ("inline babel call name", "call_{M}()"),
    (
        "results of an inline call with :exports none",
        "call_fn[:exports none]() {{{results(={M}=)}}}",
    ),
    (
        "results of an inline call with :exports code",
        "call_fn()[:exports code] {{{results(={M}=)}}}",
    ),
    ("an HTML export snippet", "@@html:<b>{M}</b>@@"),
    ("a LaTeX export snippet", "@@latex:\\textbf{{M}}@@"),
];

/// Containers; `{X}` becomes the hidden construct. Every one is a whole
/// document piece, and the node ids are fixed (no marker in them).
const CONTAINERS: &[(&str, &str)] = &[
    ("paragraph", "Text {X} end.\n"),
    (
        "external link description",
        "See [[https://example.com/ok][{X}]] here.\n",
    ),
    (
        "external link description with text",
        "See [[https://example.com/ok][ok {X}]] here.\n",
    ),
    ("resolved KB link description", "See [[id:shown][{X}]] here.\n"),
    ("unresolved KB link description", "See [[id:nowhere][{X}]] here.\n"),
    ("fuzzy link description", "See [[nowhere][{X}]] here.\n"),
    ("heading", "* Head {X}\n"),
    ("heading that is only the construct", "* {X}\n"),
    (
        "heading node title",
        "* Node {X}\n:PROPERTIES:\n:ID: n1\n:END:\nbody\n",
    ),
    (
        "heading node title that is only a link",
        "* [[id:nowhere][{X}]]\n:PROPERTIES:\n:ID: n2\n:END:\nbody\n",
    ),
    (
        "heading node title that is only a resolved link",
        "* [[id:shown][{X}]]\n:PROPERTIES:\n:ID: n3\n:END:\nbody\n",
    ),
    (
        "heading title link, then a same-page link to it",
        "* Linked [[https://example.com/ok][{X}]]\nSee [[*Linked [[https://example.com/ok][{X}]]]].\n",
    ),
    ("item tag", "- {X} :: body\n- other :: more\n"),
    (
        "item tag link",
        "- [[id:nowhere][{X}]] :: body\n- other :: more\n",
    ),
    (
        "item node title",
        "- {X} :: body\n  :PROPERTIES:\n  :ID: i1\n  :END:\n",
    ),
    // A table cell allows no babel object (Org: `src_sh` there is a
    // subscript), so a construct in one is in a link description in it.
    ("table cell", "| a | [[id:nowhere][{X}]] |\n| b | c |\n"),
    ("caption", "#+CAPTION: {X} [[id:nowhere][{X}]]\n| a |\n"),
    (
        "footnote definition",
        "See[fn:d1].\n\n[fn:d1] Note {X} [[id:nowhere][{X}]].\n",
    ),
    (
        "inline footnote reference",
        "See[fn:: Note {X} [[id:nowhere][{X}]]].\n",
    ),
    (
        "property value",
        "* P\n:PROPERTIES:\n:ID: p1\n:PROP: v {X}\n:LNK: [[id:nowhere][{X}]]\n:END:\n",
    ),
    (
        "alias",
        "* A\n:PROPERTIES:\n:ID: a1\n:ALIASES: al {X}, other\n:END:\n",
    ),
    (
        "file title",
        ":PROPERTIES:\n:ID: f1\n:END:\n#+TITLE: T {X}\n",
    ),
];

/// The called block of the inline calls, visible and harmless; after the
/// container, which may need to start the file.
const POSTAMBLE: &str = "\n#+NAME: fn\n#+begin_src sh :exports both\necho visible\n#+end_src\n";

fn resolver() -> Resolver {
    Resolver::default()
        .with_target(
            "shown",
            Resolved::new("/shown.html").with_title("Shown title"),
        )
        .with_target("untitled", Resolved::new("/untitled.html"))
}

/// Every output of `doc`, one string per surface.
fn outputs(doc: &str) -> Vec<(String, String)> {
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    let r = resolver();
    let mut out = Vec::new();
    let option_sets = [
        RenderOptions::default(),
        RenderOptions::default()
            .with_property_drawers(true)
            .with_images(true),
        RenderOptions::default().with_headline_ids(HeadlineIds::Slug),
    ];
    for (i, opts) in option_sets.iter().enumerate() {
        let rendered = render(&d, &sem, opts, &r);
        out.push((format!("hast/{i}"), rendered.to_json(TreeFormat::Hast)));
        out.push((format!("html/{i}"), rendered.to_json(TreeFormat::Html)));
    }
    out.push(("text".into(), text(&d, &sem)));
    for n in &sem.nodes {
        let t = node_text(&d, &sem, &n.id).unwrap_or_default();
        out.push((format!("node_text {}", n.id), t));
    }
    out
}

#[test]
fn no_hidden_inline_construct_reaches_any_output_in_any_container() {
    let mut leaks = Vec::new();
    for (ci, (cname, container)) in CONTAINERS.iter().enumerate() {
        for (hi, (hname, hidden)) in HIDDEN.iter().enumerate() {
            let m = format!("QZH{ci:02}x{hi:02}QZH");
            let x = hidden.replace("{M}", &m);
            let doc = format!("{}{POSTAMBLE}", container.replace("{X}", &x));
            for (surface, s) in outputs(&doc) {
                if s.contains(&m) {
                    leaks.push(format!("{hname} in {cname} -> {surface}"));
                }
            }
        }
    }
    leaks.dedup_by(|a, b| a.split(" -> ").next() == b.split(" -> ").next());
    assert!(
        leaks.is_empty(),
        "{} leaks:\n{}",
        leaks.len(),
        leaks.join("\n")
    );
}

/// The test is not vacuous: the same containers with a VISIBLE construct
/// (an inline src block exported as code) do show the marker, in every
/// container that is rendered.
#[test]
fn a_visible_construct_in_every_container_is_shown() {
    for (ci, (cname, container)) in CONTAINERS.iter().enumerate() {
        let m = format!("QZV{ci:02}QZV");
        let x = format!("src_sh[:exports code]{{{m}}}");
        let doc = format!("{}{POSTAMBLE}", container.replace("{X}", &x));
        let shown = outputs(&doc).iter().any(|(_, s)| s.contains(&m));
        assert!(
            shown,
            "{cname}: the visible marker is shown nowhere:\n{doc}"
        );
    }
}

/// A link whose description renders empty renders as if it had none: the
/// URL of an external link, the resolver's title of a resolved KB link, the
/// placeholder of an unresolved one. Never the description's source.
#[test]
fn a_link_whose_description_renders_empty_shows_what_a_bare_link_shows() {
    let hide = "src_sh[:exports none]{echo SEC} {{{results(=SEC=)}}}";
    let cases = [
        (
            format!("[[https://example.com/a][{hide}]]"),
            "<a class=\"org-link\" href=\"https://example.com/a\">https://example.com/a</a>",
        ),
        (
            format!("[[id:shown][{hide}]]"),
            "href=\"/shown.html\" data-org-rel=\"references\" data-org-w=\"1\" data-org-c=\"1\">Shown title</a>",
        ),
        (
            format!("[[id:nowhere][{hide}]]"),
            "<span class=\"org-unresolved\">[?]</span>",
        ),
        // Resolved, but the resolver gave no title: never the id as written.
        (
            format!("[[id:untitled][{hide}]]"),
            "href=\"/untitled.html\" data-org-rel=\"references\" data-org-w=\"1\" data-org-c=\"1\">[?]</a>",
        ),
        (
            "[[id:nowhere][@@html:<b>SEC</b>@@]]".to_string(),
            "<span class=\"org-unresolved\">[?]</span>",
        ),
    ];
    for (link, want) in cases {
        let doc = format!("Text {link} end.\n");
        let d = parse(&doc, &ParseOptions::default());
        let sem = semantics(&d);
        let html = mae_org::render_html(&d, &sem, &RenderOptions::default(), &resolver());
        assert!(html.contains(want), "{doc}\n{html}");
        assert!(!html.contains("SEC"), "{doc}\n{html}");
    }
}

/// A same-page link without a description (or whose description renders
/// empty) shows what it lands on, as rendered: the heading's title, a KB
/// node's title. Never its own path, which is source text.
#[test]
fn a_same_page_link_shows_the_title_it_lands_on() {
    let doc = "* Kinds src_sh{echo SEC} here\n:PROPERTIES:\n:CUSTOM_ID: kinds\n:ID: k1\n:END:\n- tag :: an item node\n  :PROPERTIES:\n  :ID: i1\n  :END:\n\nSee [[*Kinds src_sh{echo SEC} here]], [[#kinds]], [[id:k1]], [[id:i1]] and [[#kinds][src_sh{SEC}]].\n";
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    let html = mae_org::render_html(&d, &sem, &RenderOptions::default(), &Resolver::default());
    assert!(!html.contains("SEC"), "{html}");
    let p = &html[html.rfind("<p>").unwrap()..];
    assert_eq!(p.matches(">Kinds here</a>").count(), 4, "{p}");
    assert!(p.contains(">tag :: an item node</a>"), "{p}");
}
