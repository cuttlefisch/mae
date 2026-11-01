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
    // Review round 3: a macro whose TEMPLATE hides its argument (Org
    // expands it, then exports what it became), defined in POSTAMBLE.
    ("macro template: inline src", "{{{mh({M})}}}"),
    ("macro template: export snippet", "{{{ms({M})}}}"),
    ("macro template: babel call", "{{{mc({M})}}}"),
    ("macro template: through another macro", "{{{mt({M})}}}"),
    ("macro template: (eval …)", "{{{me({M})}}}"),
    ("macro template: a cycle", "{{{ca({M})}}}"),
    (
        "macro template: unclosed, closed by the text after",
        "{{{mu({M})}}}}",
    ),
    ("macro: undefined", "{{{nosuch({M})}}}"),
    // Round 3 (the second reviewer): arguments that form a hidden
    // construct only once substituted into `$1$2` (no argument alone
    // holds even the start of one).
    (
        "macro template: arguments joined by the template",
        "{{{cat(src,_sh[:exports none]{echo {M}})}}}",
    ),
    (
        "macro template: the reviewer's split arguments",
        "{{{cat(src_sh[:exports none],{echo {M}})}}}",
    ),
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
    // Round 3 (the second reviewer): a property value dropped hidden
    // objects at its top level only; nested in another object they were
    // shown as written.
    (
        "property value, nested in other objects",
        "* PN\n:PROPERTIES:\n:ID: pn\n:A: *bold {X}*\n:B: [cite:@a {X}]\n:C: {{{mp({X})}}}\n:D: _u {X}_\n:E: [[https://example.com/ok][*d {X}*]]\n:ALIASES: [[https://example.com/ok][*d {X}*]]\n:END:\n",
    ),
    (
        "file title",
        ":PROPERTIES:\n:ID: f1\n:END:\n#+TITLE: T {X}\n",
    ),
    ("emphasis", "*bold {X} bold* and /it {X}/ and _u {X}_ and +s {X}+\n"),
    ("sub- and superscript", "x^{a {X}} and y_{b {X}}\n"),
    (
        "image link description",
        "See [[https://example.com/a.png][{X}]] here.\n",
    ),
    (
        "emphasis in a link description",
        "See [[https://example.com/ok][*b {X}*]] here.\n",
    ),
    ("TODO heading with priority and tags", "* TODO [#A] {X} :tag:\n"),
    ("quote block", "#+begin_quote\nQ {X}\n#+end_quote\n"),
    ("center block", "#+begin_center\nC {X}\n#+end_center\n"),
    ("verse block", "#+begin_verse\nV {X}\n#+end_verse\n"),
    ("special block", "#+begin_foo\nS {X}\n#+end_foo\n"),
    ("checkbox items", "- [ ] item {X}\n  1. nested {X}\n"),
    ("table header", "| h [[id:nowhere][{X}]] |\n|---|\n| b |\n"),
    ("labelled inline footnote", "Footnote[fn:lbl:inline {X}] here.\n"),
    ("sub-heading", "* Parent\n** Child {X}\nbody\n"),
    // Review round 2: Org expands a macro and prints its argument, and so
    // does MAE (round 4); a citation is printed as source, so one holding a
    // hidden construct must not print it (fail closed).
    ("macro argument", "#+MACRO: m $1\nSee {{{m({X})}}} end.\n"),
    ("citation prefix", "See [cite:pre {X} @key suf] end.\n"),
    ("citation suffix", "See [cite:@key suf {X}] end.\n"),
];

/// Containers that hide more than Org by design (a citation holding any
/// babel object or snippet shows only its keys): a VISIBLE construct in
/// them is not shown either.
const FAIL_CLOSED: &[&str] = &["citation prefix", "citation suffix"];

/// The called block of the inline calls, visible and harmless; after the
/// container, which may need to start the file.
const POSTAMBLE: &str = "\n#+NAME: fn\n#+begin_src sh :exports both\necho visible\n#+end_src\n\n\
#+MACRO: mh src_sh[:exports none]{echo $1}\n\
#+MACRO: ms @@latex:\\textbf{$1}@@\n\
#+MACRO: mr {{{results($1)}}}\n\
#+MACRO: mc call_fn($1)\n\
#+MACRO: mt <{{{mh($1)}}}>\n\
#+MACRO: me (eval (concat \"$1\"))\n\
#+MACRO: ca {{{cb($1)}}}\n\
#+MACRO: cb {{{ca($1)}}}\n\
#+MACRO: mu src_sh[:exports none]{echo $1\n\
#+MACRO: mp <$1>\n\
#+MACRO: mq [{{{mp($1)}}}]\n\
#+MACRO: cat $1$2\n";

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
            // Before a citation's key, `@@html:…@@` IS the key (Org's key
            // syntax takes `@` and `:`): no snippet there to hide, and Org
            // prints no key either (`??`).
            if *cname == "citation prefix" && hname.contains("export snippet") {
                continue;
            }
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
        if FAIL_CLOSED.contains(cname) {
            continue;
        }
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

/// Where `:exports` is set, review round 2: an alias was rendered with no
/// place in the document, so its inline src block read the FILE's
/// `header-args` (`:exports code`) instead of its heading's (`:exports
/// none`) and showed the code. Every scope Org reads a block's `:exports`
/// from, against a broader or unrelated scope that says the opposite, with
/// the construct in every container of the scoped heading.
const SCOPES: &[(&str, &str, &str, &str)] = &[
    // (name, file keyword lines, outer heading drawer lines, inner heading
    // drawer lines)
    (
        "#+PROPERTY",
        "#+PROPERTY: header-args :exports none\n",
        "",
        "",
    ),
    (
        "heading drawer",
        "#+PROPERTY: header-args :exports both\n",
        "",
        ":header-args: :exports none\n",
    ),
    (
        "inherited from an ancestor heading",
        "#+PROPERTY: header-args :exports both\n",
        ":header-args: :exports none\n",
        "",
    ),
    (
        "header-args:LANG in the drawer",
        "#+PROPERTY: header-args :exports both\n",
        "",
        ":header-args:sh: :exports none\n",
    ),
    (
        "#+PROPERTY header-args:LANG",
        "#+PROPERTY: header-args :exports both\n#+PROPERTY: header-args:sh :exports none\n",
        "",
        "",
    ),
];

/// Containers under the scoped heading `Inner`; `{X}` is the construct.
const SCOPED: &[(&str, &str)] = &[
    ("title", "TITLE"),
    ("alias", "ALIAS"),
    ("property value", "PROP"),
    ("paragraph", "Para {X} end.\n"),
    ("external link", "See [[https://example.com/ok][{X}]].\n"),
    ("resolved KB link", "See [[id:shown][{X}]].\n"),
    ("unresolved KB link", "See [[id:nowhere][{X}]].\n"),
    ("item tag", "- {X} :: body\n- other :: more\n\n"),
    (
        "item node title",
        "- {X} :: body\n  :PROPERTIES:\n  :ID: si\n  :END:\n\n",
    ),
    ("table cell link", "| [[id:nowhere][{X}]] |\n\n"),
    ("caption", "#+CAPTION: {X}\n| a |\n\n"),
    (
        "footnote definition",
        "Ref[fn:s1].\n\n[fn:s1] Note {X}.\n\n",
    ),
    ("inline footnote", "Ref[fn:: note {X}].\n"),
    ("macro argument", "{{{m({X})}}}\n"),
    ("citation", "[cite:pre {X} @k]\n"),
    ("sub-heading", "SUB"),
];

/// The scoped document: `x(i)` is container `i`'s construct.
fn scoped_doc(scope: (&str, &str, &str), x: &dyn Fn(usize) -> String) -> String {
    let (file, outer, inner) = scope;
    let at = |name: &str| SCOPED.iter().position(|c| c.1 == name).unwrap();
    let mut body = String::new();
    for (i, (_, c)) in SCOPED.iter().enumerate() {
        if !matches!(*c, "TITLE" | "ALIAS" | "PROP" | "SUB") {
            body.push_str(&c.replace("{X}", &x(i)));
        }
    }
    format!(
        "{file}#+MACRO: m $1\n* Other\n:PROPERTIES:\n:header-args: :exports both\n:END:\nother\n\
         * Outer\n:PROPERTIES:\n{outer}:END:\n\
         ** Inner {title}\n:PROPERTIES:\n:ID: inner\n{inner}:ALIASES: al {alias}\n:PROP: v {prop}\n:END:\n\
         {body}*** Sub {sub}\nsub body\n* After\n:PROPERTIES:\n:header-args: :exports both\n:END:\n",
        title = x(at("TITLE")),
        alias = x(at("ALIAS")),
        prop = x(at("PROP")),
        sub = x(at("SUB")),
    )
}

#[test]
fn hidden_constructs_follow_their_own_scope_in_every_container() {
    let mut leaks = Vec::new();
    for (si, (sname, file, outer, inner)) in SCOPES.iter().enumerate() {
        let marker = |i: usize| format!("QZS{si}x{i:02}QZS");
        let x = |i: usize| {
            let m = marker(i);
            format!("src_sh{{echo {m}}} {{{{{{results(={m}=)}}}}}}")
        };
        let doc = scoped_doc((file, outer, inner), &x);
        for (surface, s) in outputs(&doc) {
            for (i, (cname, _)) in SCOPED.iter().enumerate() {
                if s.contains(&marker(i)) {
                    leaks.push(format!("{sname}: {cname} -> {surface}"));
                }
            }
        }
    }
    leaks.sort();
    leaks.dedup_by(|a, b| a.split(" -> ").next() == b.split(" -> ").next());
    assert!(
        leaks.is_empty(),
        "{} leaks:\n{}",
        leaks.len(),
        leaks.join("\n")
    );
}

/// Not vacuous: with the heading's scope SHOWING code (`:exports both`)
/// under a file that hides it, every container but the fail-closed ones
/// shows its marker, so each is read at the heading's place.
#[test]
fn a_construct_shown_by_its_own_scope_is_shown_in_every_container() {
    let x = |i: usize| format!("src_sh{{echo QZW{i:02}QZW}}");
    let doc = scoped_doc(
        (
            "#+PROPERTY: header-args :exports none\n",
            "",
            ":header-args: :exports both\n",
        ),
        &x,
    );
    let outs = outputs(&doc);
    for (i, (cname, _)) in SCOPED.iter().enumerate() {
        // Fail closed by design: a citation holding any babel object shows
        // none of it, and a property value never shows one
        // (property-value-babel).
        if matches!(*cname, "citation" | "property value") {
            continue;
        }
        let m = format!("QZW{i:02}QZW");
        assert!(
            outs.iter().any(|(_, s)| s.contains(&m)),
            "{cname}: shown nowhere:\n{doc}"
        );
    }
}

/// The reviewer's inputs (round 2), as written: Emacs 30.2 exports the
/// macro lines as `See and end.` and prints the citation's inline src code
/// and snippet HTML; MAE expands the macros as Emacs does, and shows none
/// of the citation's.
#[test]
fn the_reviewers_macro_and_citation_show_none_of_their_hidden_text() {
    let doc = "#+MACRO: m $1\nSee {{{m(src_sh[:exports none]{echo SECRET6})}}} and {{{m(@@latex:SECRET7@@)}}} end.\n\nSee [cite:pre src_sh[:exports none]{echo SECRET8} @key suf @@html:SECRET9@@] end.\n";
    for (surface, s) in outputs(doc) {
        assert!(!s.contains("SECRET"), "{surface}:\n{s}");
    }
    let d = parse(doc, &ParseOptions::default());
    let html = mae_org::render_html(&d, &semantics(&d), &RenderOptions::default(), &resolver());
    assert!(html.contains("<p>See and  end.</p>"), "{html}");
    assert!(
        html.contains("<span class=\"org-citation\">[cite:@key]</span>"),
        "{html}"
    );
}

/// The positive twin of the template rows: a macro whose template shows
/// its argument (`<$1>`, through another macro, a results macro, a join)
/// shows it in every container that is not fail-closed.
#[test]
fn a_macro_whose_template_shows_its_argument_is_shown() {
    for (ci, (cname, container)) in CONTAINERS.iter().enumerate() {
        // Org expands no macro in a property value (an alias is one); one
        // left there shows nothing.
        if FAIL_CLOSED.contains(cname)
            || cname.starts_with("property value")
            || cname.starts_with("alias")
        {
            continue;
        }
        for (k, x) in [
            "{{{mp({M})}}}",
            "{{{mq({M})}}}",
            "{{{mr({M})}}}",
            "{{{cat(<,{M}>)}}}",
        ]
        .iter()
        .enumerate()
        {
            // In another macro's arguments, which end at the first `)}}}`
            // and split at its commas, `{{{cat(<,M>)}}}` is cut in two (as
            // in Org): `{{{m({{{cat(<,M>)}}})}}}` keeps only `<`.
            if *cname == "macro argument" && x.contains(',') {
                continue;
            }
            let m = format!("QZP{ci:02}x{k}QZP");
            let doc = format!(
                "{}{POSTAMBLE}",
                container.replace("{X}", &x.replace("{M}", &m))
            );
            let shown = outputs(&doc).iter().any(|(_, s)| s.contains(&m));
            assert!(shown, "{cname}, {x}: shown nowhere:\n{doc}");
        }
    }
}

/// Round 3, against Emacs 30.2 (`emacs -Q`, ox-html): each export below,
/// and which definition Org uses (the first `#+MACRO:` of a name, names
/// compared case-insensitively; `#+TITLE:` over a `title` macro).
#[test]
fn macro_templates_hide_as_org_expands_them() {
    let cases: &[(&str, &str, &str)] = &[
        // (document, Emacs's paragraph, what MAE must not show)
        (
            "#+MACRO: m src_sh[:exports none]{echo $1}\n* T\nSee {{{m(SECRETA)}}} end.\n",
            "See end.",
            "SECRETA",
        ),
        (
            "#+MACRO: d src_sh[:exports none]{echo $1}\n#+MACRO: D <$1>\nA {{{d(QD1)}}} B\n",
            "A B",
            "QD1",
        ),
        (
            "#+TITLE: x src_sh[:exports none]{echo $1}\n#+MACRO: title <$1>\nA {{{title(QT1)}}} B\n",
            "A x B",
            "QT1",
        ),
        (
            "#+MACRO: u src_sh[:exports none]{echo $1\nA {{{u(QU1)}}}} B\n",
            "A B",
            "QU1",
        ),
        (
            "#+MACRO: cat $1$2\nA {{{cat(src_sh[:exports none],{echo SECRETTWO})}}} B {{{cat(src,_sh[:exports none]{echo SECRETSPLIT})}}} C\n",
            "A B C",
            "SECRET",
        ),
    ];
    for (doc, _emacs, secret) in cases {
        for (surface, s) in outputs(doc) {
            assert!(!s.contains(secret), "{surface}:\n{doc}\n{s}");
        }
    }
    // The first definition wins: with the plain one first, Emacs shows
    // `A <QD2> B`, and MAE shows the argument (as source).
    let doc = "#+MACRO: d <$1>\n#+MACRO: D src_sh[:exports none]{echo $1}\nA {{{d(QD2)}}} B\n";
    assert!(outputs(doc).iter().any(|(_, s)| s.contains("QD2")), "{doc}");
    // Emacs: `C <QPOS> D a,bQE E`: a plain join shows, an escaped comma
    // is one argument.
    let doc = "#+MACRO: cat $1$2\nC {{{cat(<,QPOS>)}}} D {{{cat(a\\,b,QE)}}} E\n";
    let outs = outputs(doc);
    assert!(outs.iter().any(|(_, s)| s.contains("QPOS")), "{doc}");
    assert!(outs.iter().any(|(_, s)| s.contains("QE")), "{doc}");
    // Expansion is bounded by the source's size, not by a depth: a chain
    // of plain templates shows its argument however long. A template
    // calling its successor twice, 12 deep (4,096 calls, each read again in
    // its paragraph as the scan reaches it), costs past the bound: the
    // document renders nothing (Emacs shows it; registered: macros).
    let chain = |n: usize| -> String {
        let mut d: String = (1..n)
            .map(|i| format!("#+MACRO: c{i} <{{{{{{c{}($1)}}}}}}>\n", i + 1))
            .collect();
        d.push_str(&format!("#+MACRO: c{n} <$1>\nA {{{{{{c1(QCHAIN)}}}}}} B\n"));
        d
    };
    let shown = |doc: &str| outputs(doc).iter().any(|(_, s)| s.contains("QCHAIN"));
    assert!(shown(&chain(5)), "{}", chain(5));
    assert!(shown(&chain(20)), "{}", chain(20));
    let mut twice: String = (1..12)
        .map(|i| {
            format!(
                "#+MACRO: t{i} {{{{{{t{0}($1)}}}}}}{{{{{{t{0}($1)}}}}}}\n",
                i + 1
            )
        })
        .collect();
    twice.push_str("#+MACRO: t12 <$1>\nA {{{t1(QCHAIN)}}} B\n");
    assert!(!shown(&twice), "{twice}");
    let d = parse(
        "#+MACRO: m src_sh[:exports none]{echo $1}\n* T\nSee {{{m(SECRETA)}}} end.\n",
        &ParseOptions::default(),
    );
    let html = mae_org::render_html(&d, &semantics(&d), &RenderOptions::default(), &resolver());
    // Emacs: `See end.` (the inline src block goes with its blank).
    assert!(html.contains("<p>See end.</p>"), "{html}");
}

/// The second reviewer's property values (round 3), as written.
#[test]
fn property_values_hide_what_their_nested_objects_hide() {
    let doc = "* H\n:PROPERTIES:\n:ID: h\n:A: *bold src_sh[:exports none]{echo SECRETBOLD}*\n:B: [cite:@a src_sh[:exports none]{echo SECRETCITE}]\n:C: {{{m(src_sh[:exports none]{echo SECRETMAC})}}}\n:D: _u @@latex:SECRETSNIP@@_\n:E: plain src_sh[:exports none]{echo HIDDENOK}\n:END:\nBody.\n";
    for (surface, s) in outputs(doc) {
        assert!(
            !s.contains("SECRET") && !s.contains("HIDDENOK"),
            "{surface}:\n{s}"
        );
    }
    let d = parse(doc, &ParseOptions::default());
    let r = render(&d, &semantics(&d), &RenderOptions::default(), &resolver());
    let a = &r.nodes[0]
        .properties
        .iter()
        .find(|(k, _)| k == "A")
        .unwrap()
        .1;
    assert_eq!(mae_org::to_html(a), "bold", "{a:?}");
    // In a value an inline src block is hidden whatever its `:exports`
    // (property-value-babel), even one its scope would show.
    let doc = "* H\n:PROPERTIES:\n:ID: h\n:header-args: :exports both\n:F: shown src_sh[:exports code]{echo QZVALUE}\n:END:\nBody src_sh[:exports code]{echo QZBODY}.\n";
    let outs = outputs(doc);
    assert!(outs.iter().any(|(_, s)| s.contains("QZBODY")), "{doc}");
    for (surface, s) in outs {
        assert!(!s.contains("QZVALUE"), "{surface}:\n{s}");
    }
}
